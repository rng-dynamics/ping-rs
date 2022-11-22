use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::event::*;
use crate::ping_output::*;
use crate::socket::*;
use crate::GenericError;
use crate::IcmpV4;
use crate::PingDataBuffer;
use crate::PingError;
use crate::PingReceiver;
use crate::PingSender;

pub type PingResult<T> = std::result::Result<T, GenericError>;

struct Inner {
    sender_halt_tx: mpsc::Sender<()>,
    sender_thread: Option<JoinHandle<()>>,
    receiver_halt_tx: mpsc::Sender<()>,
    receiver_thread: Option<JoinHandle<()>>,
    ping_output_rx: PingOutputReceiver,
}

#[derive(Clone, PartialEq, Debug)]
pub enum State {
    New,
    Running,
    Halted,
}

pub struct PingRs {
    states: Vec<State>,
    inner: Option<Inner>,
}

impl Drop for PingRs {
    fn drop(&mut self) {
        if !self.is_in_state(State::Halted) || self.inner.is_some() {
            panic!("you must call halt on PingRs to clean it up");
        }
    }
}

impl PingRs {
    pub fn new(channel_size: usize) -> Self {
        Self {
            states: vec![State::New],
            inner: None,
        }
    }

    pub fn run(&mut self, ips: &[Ipv4Addr], count: u16, interval: Duration) -> PingResult<()> {
        if !self.is_in_state(State::New) {
            return Err(PingError {
                message: "cannot run() PingRunner when it is not in state New".to_string(),
            }
            .into());
        }

        let mut deque = VecDeque::<Ipv4Addr>::new();
        for ip in ips {
            deque.push_back(*ip);
        }

        let icmpv4 = std::sync::Arc::new(IcmpV4::create());
        let socket = Arc::new(create_socket2_dgram_socket(Duration::from_millis(2000))?);

        let (send_sync_event_tx, send_sync_event_rx) = ping_send_sync_event_channel();
        let (receive_event_tx, receive_event_rx) = ping_receive_event_channel();
        let (send_event_tx, send_event_rx) = ping_send_event_channel();
        let (ping_output_tx, ping_output_rx) = ping_output_channel();

        let ping_sender = PingSender::new(icmpv4.clone(), socket.clone(), send_event_tx);
        let ping_receiver = PingReceiver::new(icmpv4, socket, receive_event_tx);
        let ping_data_buffer = PingDataBuffer::new(send_event_rx, receive_event_rx, ping_output_tx);

        let (sender_halt_tx, sender_halt_rx) = mpsc::channel::<()>();
        let sender_thread = Self::start_sender_thread(
            ping_sender,
            sender_halt_rx,
            count,
            deque.into(),
            send_sync_event_tx,
            interval,
        );

        let (receiver_halt_tx, receiver_halt_rx) = mpsc::channel::<()>();
        let receiver_thread = Self::start_receiver_thread(
            ping_data_buffer,
            ping_receiver,
            receiver_halt_rx,
            send_sync_event_rx,
        );

        self.inner = Some(Inner {
            sender_halt_tx,
            sender_thread: Some(sender_thread),
            receiver_halt_tx,
            receiver_thread: Some(receiver_thread),
            ping_output_rx,
        });
        self.states.push(State::Running);
        Ok(())
    }

    pub fn next_ping_output(&self) -> PingResult<PingOutput> {
        if !self.is_in_state(State::Running) {
            return Err(PingError {
                message: "cannot next_ping_output() when PingRunner is not in state Running"
                    .to_string(),
            }
            .into());
        }
        let inner = self.inner.as_ref().expect("logic error");
        Ok(inner.ping_output_rx.recv()?)
    }

    pub fn halt(&mut self) -> std::thread::Result<()> {
        if self.is_in_state(State::Halted) {
            return Ok(());
        }
        if let Some(mut inner) = self.inner.take() {
            // mpsc::Sender::send() returns error only if mpsc::Receiver is closed.
            let _maybe_err_1 = inner.sender_halt_tx.send(());
            let _maybe_err_2 = inner.receiver_halt_tx.send(());

            let join_result_1 = match inner.sender_thread.take() {
                Some(handle) => handle.join(),
                None => Ok(()),
            };
            let join_result_2 = match inner.receiver_thread.take() {
                Some(handle) => handle.join(),
                None => Ok(()),
            };

            if let Err(e) = join_result_1 {
                return Err(e.into());
            }
            if let Err(e) = join_result_2 {
                return Err(e.into());
            }
        }

        self.states.push(State::Halted);
        Ok(())
    }

    pub fn get_states(&self) -> Vec<State> {
        self.states.clone()
    }

    fn is_in_state(&self, state: State) -> bool {
        match self.states.last() {
            None => false,
            Some(self_state) => *self_state == state,
        }
    }

    fn start_receiver_thread(
        mut ping_data_buffer: PingDataBuffer,
        ping_receiver: PingReceiver<socket2::Socket>,
        halt_rx: mpsc::Receiver<()>,
        ping_send_sync_event_rx: mpsc::Receiver<PingSentSyncEvent>,
    ) -> JoinHandle<()> {
        std::thread::spawn(move || {
            'outer: loop {
                // (1) Wait for sync-event from PingSender.
                let ping_sent_sync_event_recv = ping_send_sync_event_rx.recv();

                if let Err(_) = ping_sent_sync_event_recv {
                    tracing::info!("mpsc::Receiver::recv() failed");
                    break 'outer;
                }

                // (2) receive ping and update ping buffer
                let receive_result = ping_receiver.receive();
                if let Err(_) = receive_result {
                    tracing::error!("PingReceiver::receive() failed");
                    break 'outer;
                }
                ping_data_buffer.update();

                // (4) check termination
                match halt_rx.try_recv() {
                    Ok(_) | Err(std::sync::mpsc::TryRecvError::Disconnected) => break 'outer,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }
            }
        })
    }

    fn start_sender_thread(
        ping_sender: PingSender<socket2::Socket>,
        halt_rx: mpsc::Receiver<()>,
        count: u16,
        ips: VecDeque<Ipv4Addr>,
        ping_send_sync_event_tx: mpsc::SyncSender<PingSentSyncEvent>,
        interval: Duration,
    ) -> JoinHandle<()> {
        std::thread::spawn(move || {
            tracing::trace!("PingSender thread start with count {}", count);
            'outer: for sequence_number in 0..count {
                tracing::trace!("PingSender outer loop start");
                for ip in &ips {
                    tracing::trace!("PingSender inner loop start");
                    if ping_sender.send_one(*ip, sequence_number).is_err() {
                        tracing::error!("PingSender::send_one() failed");
                        break 'outer;
                    }
                    // (2.2) Dispatch sync event.
                    if ping_send_sync_event_tx.send(PingSentSyncEvent).is_err() {
                        tracing::error!("mpsc::SyncSender::send() failed");
                        break 'outer;
                    }
                    tracing::trace!("PingSender published SYNC-Event");

                    // (3) Check termination.
                    match halt_rx.try_recv() {
                        Ok(_) | Err(std::sync::mpsc::TryRecvError::Disconnected) => break 'outer,
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    }
                }
                if sequence_number < count - 1 {
                    // (4) Sleep according to configuration
                    tracing::trace!("PingSender will sleep");
                    std::thread::sleep(interval);
                }
            }
            tracing::trace!("PingSender thread end");
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_localhost_succeed() {
        let channel_size = 8;
        let ips = [Ipv4Addr::new(127, 0, 0, 1)];
        let count = 1;

        let mut ping = PingRs::new(channel_size);

        ping.run(&ips, count, Duration::from_secs(1)).unwrap();
        let output = ping.next_ping_output();
        println!("output received: {:?}", output);
        let halt_result = ping.halt();

        assert!(output.is_ok());
        assert!(halt_result.is_ok());
    }

    #[test]
    fn entity_states_are_correct() {
        let channel_size = 8;
        let ips = [Ipv4Addr::new(127, 0, 0, 1)];
        let count = 1;

        let mut ping = PingRs::new(channel_size);
        assert!(vec![State::New] == ping.get_states());
        ping.run(&ips, count, Duration::from_secs(1)).unwrap();
        assert!(vec![State::New, State::Running] == ping.get_states());
        ping.halt().unwrap();
        assert!(vec![State::New, State::Running, State::Halted] == ping.get_states());
    }

    #[test]
    #[should_panic(expected = "you must call halt on PingRs to clean it up")]
    fn not_calling_halt_may_panic_on_drop() {
        let channel_size = 8;
        let ping = PingRs::new(channel_size);
        drop(ping);
    }

    #[test]
    fn calling_start_after_halt_is_ignored() {
        let channel_size = 8;
        let ips = [Ipv4Addr::new(127, 0, 0, 1)];
        let count = 1;

        let mut ping = PingRs::new(channel_size);
        ping.halt().unwrap();
        let run_result = ping.run(&ips, count, Duration::from_secs(1));

        assert!(run_result.is_err());
        assert!(vec![State::New, State::Halted] == ping.get_states());
    }

    #[test]
    fn calling_start_a_second_time_is_ignored() {
        let channel_size = 8;
        let ips_127_0_0_1 = [Ipv4Addr::new(127, 0, 0, 1)];
        let ips_254_254_254_254 = [Ipv4Addr::new(254, 254, 254, 254)];
        let count = 1;

        let mut ping = PingRs::new(channel_size);
        let run_result_1 = ping.run(&ips_127_0_0_1, count, Duration::from_secs(1));
        let run_result_2 = ping.run(&ips_254_254_254_254, count, Duration::from_secs(1));

        assert!(run_result_1.is_ok());
        assert!(run_result_2.is_err());

        ping.halt().unwrap();
    }
}
