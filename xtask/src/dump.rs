shadow_rs::shadow!(build);

/// 打印仓库和编译信息。
pub fn dump_config() {
    println!(
        "\
* ------------------------
| Build
|   Host   {os}
|   Mode   {mode}
|   Rustc  {rustc}
|   Cargo  {cargo}
|   Time   {build_time}
* ------------------------
| Version Control
|   Branch {branch} ({rev})
|   Author {name} <{email}>
|   Time   {vc_time}
* ------------------------",
        os = build::BUILD_OS,
        mode = build::BUILD_RUST_CHANNEL,
        rustc = build::RUST_VERSION,
        cargo = build::CARGO_VERSION,
        build_time = build::BUILD_TIME,
        branch = build::BRANCH,
        name = build::COMMIT_AUTHOR,
        email = build::COMMIT_EMAIL,
        rev = build::SHORT_COMMIT,
        vc_time = build::COMMIT_DATE,
    );
}
