# stellar-query

Component-side companion to issue #5. Calls the new
`host::get_stellar_chain_config` WIT function, logs the result, then
panics with `unimplemented!` — actually querying Soroban from a
component requires a wasip2-compatible RPC client we don't ship yet.

The matching e2e test (`stellar_stellar_query`) is wired into the
default test config so a local `cargo test -p warpdrive-tests` run
fails with the panic message. CI configs (`warpdrive-tests-ci-basic`
/ `-ci-complete`) deliberately don't include this test so CI passes.

## (Re)build the component

In repo root:

```bash
task wasi-build COMPONENT=stellar-query
```

## Run via the local test config

```bash
cargo test -p warpdrive-tests
```

The test will fail at the `unimplemented!` site; that's expected
until the follow-up PR that adds the Soroban client.
