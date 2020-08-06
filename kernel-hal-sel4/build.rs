fn main() {
    println!("cargo:rustc-link-search=../zcboot-sel4/zc_loader/");
    println!("cargo:rerun-if-changed=../zcboot-sel4/zc_loader/libzc_loader.a");
}