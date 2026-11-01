//! 生成 AI への問い合わせを別スレッドで走らせ、画面を止めずに結果を待つ
//!
//! 問い合わせには数秒〜数分かかるので、UI のシステムからは [`LlmJob::start`] で始め、
//! 毎フレーム [`LlmJob::poll`] で終わったかを見る。Bevy・egui に依存しない。

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use plantwright_core::assistant::llm::{LlmClient, LlmError};

/// 実行中の問い合わせ1つ。`T` は返事を読み取った結果。
pub struct LlmJob<T> {
    /// Bevy のリソースに入れられるよう Mutex で包む (Receiver は Sync でない)
    receiver: Mutex<Receiver<Result<T, LlmError>>>,
    started: Instant,
}

impl<T: Send + 'static> LlmJob<T> {
    /// `work` を別スレッドで実行する。`work` の中で `client` に問い合わせ、返事を読み取る。
    pub fn start(
        client: Arc<dyn LlmClient>,
        work: impl FnOnce(&dyn LlmClient) -> Result<T, LlmError> + Send + 'static,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(work(client.as_ref()));
        });
        Self {
            receiver: Mutex::new(receiver),
            started: Instant::now(),
        }
    }

    /// 終わっていれば結果を返す。まだなら None。
    pub fn poll(&self) -> Option<Result<T, LlmError>> {
        let Ok(receiver) = self.receiver.lock() else {
            return Some(Err(LlmError::InvalidResponse(
                "問い合わせの処理が途中で止まりました".into(),
            )));
        };
        match receiver.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(LlmError::InvalidResponse(
                "問い合わせの処理が途中で止まりました".into(),
            ))),
        }
    }

    /// 始めてからの秒数 (「考え中… 12 秒」の表示用)
    pub fn elapsed_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plantwright_core::assistant::llm::{LlmRequest, ScriptedLlm};
    use std::time::Duration;

    #[test]
    fn runs_work_in_background_and_returns_result() {
        let client: Arc<dyn LlmClient> = Arc::new(ScriptedLlm::reply("42"));
        let job = LlmJob::start(client, |llm| {
            llm.complete(&LlmRequest::new("", "?"))
                .map(|s| s.parse::<i32>().unwrap())
        });
        let mut result = None;
        for _ in 0..200 {
            result = job.poll();
            if result.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(result, Some(Ok(42)));
    }
}
