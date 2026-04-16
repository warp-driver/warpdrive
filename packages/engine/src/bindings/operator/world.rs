// https://docs.rs/wasmtime/latest/wasmtime/component/macro.bindgen.html#options-reference

use wasmtime::component::bindgen;

bindgen!({
    world: "warpdrive-world",
    path: "../../wit-definitions/vector/wit",
    with: {
        "wasi:keyvalue/store.bucket": crate::backend::wasi_keyvalue::bucket_keys::KeyValueBucket,
        "wasi:keyvalue/atomics.cas": crate::backend::wasi_keyvalue::atomics::KeyValueCas,
    },
    exports: {
        default: async,
    },
});
