// The bindings generator, in-crate so the build script can run it without a
// separately versioned `uniffi-bindgen` install drifting from the `uniffi`
// runtime this crate links. Invoked in library mode against the built
// staticlib (see build/build-xcframework.sh).
fn main() {
    uniffi::uniffi_bindgen_main()
}
