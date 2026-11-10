//! Generates the Swift bindings, the C header and the module map from
//! the built library.

fn main() {
    uniffi::uniffi_bindgen_swift()
}
