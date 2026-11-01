//! モデリングで作った形状 (B-rep) を `.plantwright` に埋め込む JSON にする
//!
//! `crate::modeling::SolidStore` の実装 ([`JsonSolids`]) と、
//! `ModeledSource` から形状を作り直すメソッド (`solid`・`to_imported`)。

use std::sync::Arc;

use crate::model::ModeledSource;
use crate::modeling::{SolidStore, shape_from_solid};
use crate::shape::{ImportNode, ImportedFile, next_shape_id};
use glam::DMat4;
use truck_modeling::Solid;

/// B-rep を JSON にして `ModeledSource` に詰める
pub struct JsonSolids;

impl SolidStore for JsonSolids {
    fn save(&self, name: String, solid: &Solid) -> Result<ModeledSource, String> {
        ModeledSource::new(name, solid)
    }

    fn load(&self, source: &ModeledSource) -> Result<Solid, String> {
        source.solid()
    }
}

/// `ModeledSource` (B-rep を JSON で持つ) と truck の `Solid` の変換
impl ModeledSource {
    pub fn new(name: String, solid: &Solid) -> Result<Self, String> {
        let json =
            serde_json::to_string(solid).map_err(|e| format!("形状を保存できません: {e}"))?;
        Ok(Self {
            id: next_shape_id(),
            name,
            json,
        })
    }

    pub fn solid(&self) -> Result<Solid, String> {
        serde_json::from_str(&self.json).map_err(|e| format!("形状を読めませんでした: {e}"))
    }

    /// モデルに取り込める形 (部品1つの木) にする
    pub fn to_imported(&self) -> Result<ImportedFile, String> {
        let shape = shape_from_solid(&self.solid()?);
        Ok(ImportedFile {
            root: ImportNode {
                name: self.name.clone(),
                product: self.name.clone(),
                placement: DMat4::IDENTITY,
                children: Vec::new(),
                shape: Some(Arc::new(shape)),
            },
            unit: String::new(),
            warnings: Vec::new(),
            animations: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Model, NodeKind, StepSource};
    use crate::modeling::{BooleanOp, add_solid, apply_boolean, cylinder};

    #[test]
    fn modeled_shape_round_trips_through_json() {
        let mut model = Model::default();
        let a = model.add_box([100.0, 100.0, 100.0], [0.0, 50.0, 0.0]);
        let cyl = cylinder(20.0, 160.0).unwrap();
        let b = add_solid(
            &mut model,
            &JsonSolids,
            "円柱".into(),
            &cyl,
            glam::DVec3::new(10.0, -30.0, 7.0),
        )
        .unwrap();
        apply_boolean(&mut model, &JsonSolids, a, b, BooleanOp::Union).unwrap();
        let NodeKind::Imported { shape, source, .. } = &model.get(a).unwrap().kind else {
            panic!();
        };
        // 保存した JSON から同じ形を作り直せる
        let StepSource::Modeled(source) = source else {
            panic!()
        };
        let again = source.to_imported().unwrap().root.shape.unwrap();
        assert_eq!(again.faces.len(), shape.faces.len());
        assert!((again.stats.max - shape.stats.max).length() < 1e-6);
    }
}
