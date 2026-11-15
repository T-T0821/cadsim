//! 形状カーネル SMLib (NVIDIA usd-brep の C++ のソース、`vendor/smlib/`) と、Rust から呼ぶための
//! 薄い層 (`cpp/kernel.cpp`) をビルドして静的にリンクする。C++17 のコンパイラが要る。

use std::path::Path;

fn sources(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{} を読めません: {e}", dir.display()))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "cpp")
                // Windows のプリコンパイル済みヘッダ用。中身は無い
                && path.file_name().is_some_and(|name| name != "StdAfx.cpp")
        })
        .collect();
    files.sort();
    files
}

/// ブラウザ向け (wasm32) のときに使う wasi-sdk の場所。`WASI_SDK_PATH` で渡す
fn wasi_sdk() -> Option<std::path::PathBuf> {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("wasm32") {
        return None;
    }
    println!("cargo:rerun-if-env-changed=WASI_SDK_PATH");
    let path = std::env::var_os("WASI_SDK_PATH").unwrap_or_else(|| {
        panic!(
            "ブラウザ向け (wasm32) のビルドには wasi-sdk が要ります。\
             展開した場所を環境変数 WASI_SDK_PATH に入れてください (docs/web-demo.md)"
        )
    });
    Some(path.into())
}

fn build(name: &str, include: &[&Path], files: &[std::path::PathBuf]) {
    let mut build = cc::Build::new();
    if let Some(sdk) = wasi_sdk() {
        // wasi-sdk の clang と C/C++ の標準ライブラリ (wasi-libc・libc++) で WebAssembly にする。
        // リンクは Rust の wasm32-unknown-unknown に混ぜ、WASI の関数はページ側の小さな JS で埋める
        build
            .compiler(sdk.join("bin/clang++"))
            .archiver(sdk.join("bin/llvm-ar"))
            .target("wasm32-wasip1")
            .flag(format!(
                "--sysroot={}",
                sdk.join("share/wasi-sysroot").display()
            ))
            .flag("-fno-exceptions")
            // SMLib はメモリ不足を例外で知らせる。例外の無い環境なので使わせない
            .define("SM_NO_EXCEPTIONS", None);
    }
    build
        .cpp(true)
        .std("c++17")
        .files(files)
        .define("NDEBUG", None)
        .define("NOMINMAX", None)
        // SM_API のヘッダは Windows では、これが無いと関数を DLL から読み込むもの (dllimport) として
        // 宣言する。ここでは DLL にせず静的にリンクするので、中身を持つ側 (dllexport) として扱わせる。
        // 無いと MSVC が「dllimport の関数は定義できない」(C2491) で止まり、Windows でビルドできない
        .define("SM_API_EXPORTS", None)
        // 開発ビルドでも最適化する (しないと形状の演算がとても遅い)
        .opt_level(2)
        .debug(false)
        .warnings(false);
    for dir in include {
        build.include(dir);
    }
    if build.get_compiler().is_like_msvc() {
        build
            .flag("/utf-8")
            .flag("/bigobj")
            .flag("/EHsc")
            .define("_CRT_SECURE_NO_WARNINGS", None)
            .define("_SCL_SECURE_NO_WARNINGS", None);
    } else {
        // 掛け算と足し算をまとめて丸めない (CPU によって結果が変わらないように)
        build.flag("-ffp-contract=off");
    }
    build.compile(name);
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let smlib = root.join("vendor/smlib/SMLib");
    let api = root.join("vendor/smlib/SM_API");
    println!("cargo:rerun-if-changed=vendor/smlib");
    println!("cargo:rerun-if-changed=cpp");

    let smlib_inc = smlib.join("inc");
    let api_inc = api.join("inc");
    // リンクの順に合わせ、使う側から先にビルドする。
    // SM_API と SMLib はどちらも StdAfx.h を持つので、自分のものが先に見つかるよう並べる
    build(
        "plantwright_kernel",
        &[&api_inc, &smlib_inc],
        &[root.join("cpp/kernel.cpp"), root.join("cpp/ops.cpp")],
    );
    build(
        "smlib_api",
        &[&api_inc, &smlib_inc],
        &sources(&api.join("src")),
    );
    build("smlib", &[&smlib_inc], &sources(&smlib.join("src")));

    if let Some(sdk) = wasi_sdk() {
        let lib = sdk.join("share/wasi-sysroot/lib/wasm32-wasip1");
        println!("cargo:rustc-link-search=native={}", lib.display());
        for name in ["c++", "c++abi", "c"] {
            println!("cargo:rustc-link-lib=static={name}");
        }
        let builtins = sdk.join("lib/clang/19/lib/wasip1");
        println!("cargo:rustc-link-search=native={}", builtins.display());
        println!("cargo:rustc-link-lib=static=clang_rt.builtins-wasm32");
    }
}
