fn main() {
    if std::env::var("TARGET").unwrap().contains("aarch64") {
        println!("cargo:rustc-env=USER_IMG=zCore/aarch64.img");
    }
}
