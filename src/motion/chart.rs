//! タイミングチャートのデータ (中身は `cadsim_core::motion::chart`) を Bevy の Resource で包んだもの

use bevy::prelude::{Deref, DerefMut, Resource};

pub use cadsim_core::motion::chart::*;

/// タイミングチャート。`cadsim_core::motion::chart::TimingChart` を Bevy の Resource にするための包み
#[derive(Resource, Clone, Debug, Default, PartialEq, Deref, DerefMut)]
pub struct TimingChart(pub cadsim_core::motion::chart::TimingChart);
