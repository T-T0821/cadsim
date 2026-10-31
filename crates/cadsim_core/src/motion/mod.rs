//! タイミングチャートと動きの計算 (ドメイン)。再生・表示・パネルはアプリ側の `motion`
//!
//! | ファイル | 役割 |
//! | --- | --- |
//! | `chart.rs` | タイミングチャートのデータと時刻表 (`Schedule`) |
//! | `profile.rs` | 加減速 (台形・サイクロイド) と電子カム表 |
//! | `pose.rs` | 時刻表 → アクチュエータ・部品の姿勢 |
//! | `kinematics.rs` | ロボットの順・逆運動学 |
//! | `robot_path.rs` / `robot_time.rs` | ロボットの経路とサイクルタイムの見積もり |
//! | `path_interference.rs` | 動作経路に沿った干渉の確認 |
//! | `sensor.rs` | センサーの検知 |

pub mod chart;
pub mod kinematics;
pub mod path_interference;
pub mod pose;
pub mod profile;
pub mod robot_path;
pub mod robot_time;
pub mod sensor;

pub use chart::{Action, Actuator, ActuatorKind, Axis, Easing, Schedule, Step, TimingChart};
pub use pose::{RigidPose, actuator_poses, actuator_poses_for, part_poses, part_poses_for};
