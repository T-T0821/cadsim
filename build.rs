//! Windows 向けのビルドでは、実行ファイル (.exe) にアプリのアイコンを埋め込む。
//! エクスプローラーやデスクトップのショートカットに plantwright のロゴが出る。
//! ほかの OS では何もしない (Mac は .app の .icns、Linux はウィンドウのアイコンを使う)。

fn main() {
    println!("cargo:rerun-if-changed=assets/icon/plantwright.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon/plantwright.ico");
    if let Err(e) = res.compile() {
        // アイコンがなくてもアプリは動くので、ビルドは止めずに知らせるだけにする
        println!("cargo:warning=実行ファイルにアイコンを埋め込めませんでした: {e}");
    }
}
