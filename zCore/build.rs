fn main() {
    // 如果需要链接 rootfs 镜像，将镜像路径设置到环境变量
    #[cfg(feature = "link-user-img")]
    println!(
        "cargo:rustc-env=USER_IMG=zCore/{}.img",
        std::env::var("TARGET").unwrap()
    );
}
