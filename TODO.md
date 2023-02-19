## TODO

- Integrate type for sequence number.
- Can we have coverage of the extern c code?
- The count in PingRunner does not seem to work. (Try the cli example.)
- Handle timeout-event in ping-data-buffer.
- Instant::now(): apply dependency inversion and mock it in tests.
- Cleanup PingError.
- TTL (needs IP packet to be sent)
- After adding TTL, reevaluate our tests/test coverage/design.
- What should happen if we receive an unexpected message (e.g., a duplicate)?
- Should we test RawSocket::recv_from? Unit test? Can we test Raw socket also in an integration test elegantly?
- More badges wirh shields.io?

## done

- Socket type (UDP, raw) should be configured by parameter.
- Code coverage badge in readme.
- Push coverage report to coveralls.
- Add clippy pedantic lints.
- Use scripts for CI whenever effective.
- Code coverage in CI. Upload coverage report to artefacts.
- Cargo clippy in CI.
- Check formatting in CI.
- Rename repo.
- Timeout is set when socket is created.
- Replace al println! by logger.
- Size of channels should be configured by parameter.
- Fix all warnings.