//! 重い処理を別スレッドで動かす
//!
//! ブラウザ版 (wasm32) にはスレッドが無い (`std::thread::spawn` は panic する) ので、その場で動かす。
//! 結果はどちらもチャネルなどで受け取る作りにしておけば、呼ぶ側は同じコードでよい。

/// `job` を別スレッドで動かす。ブラウザ版ではその場で動かし終えてから返る
pub fn spawn(job: impl FnOnce() + Send + 'static) {
    #[cfg(target_arch = "wasm32")]
    job();
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(job);
}
