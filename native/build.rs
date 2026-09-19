fn main() {
    println!("cargo:rerun-if-env-changed=BLACKGLASS_SOURCE_SHA256");
}
