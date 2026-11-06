//! `cpp/kernel.cpp` の C の関数と構造体。並びと型は C++ 側と同じにする。
//!
//! ここの関数は直接呼ばず、ロックを取ってから呼ぶ `kernel::brep` などを通す。

use std::ffi::{c_char, c_void};

/// SMLib の `SmBrep*`
pub type BrepPtr = *mut c_void;
/// 断面の線 (C++ の PwLines*)
pub type LinesPtr = *mut c_void;
/// `PwMesh*` (テッセレーションの結果)
pub type MeshPtr = *mut c_void;

/// SMLib が成功を表す値 (`SM_SUCCESS`)
pub const SUCCESS: i32 = 1000;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PwCurve {
    pub kind: i32,
    pub p0: [f64; 3],
    pub p1: [f64; 3],
    pub center: [f64; 3],
    pub xaxis: [f64; 3],
    pub yaxis: [f64; 3],
    pub r1: f64,
    pub r2: f64,
    pub a0: f64,
    pub a1: f64,
    pub degree: i32,
    pub ncp: i32,
    pub cp: *const f64,
    pub weights: *const f64,
    pub nknots: i32,
    pub knots: *const f64,
    pub mults: *const i32,
    pub full: i32,
    pub reverse: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PwSurface {
    pub kind: i32,
    pub origin: [f64; 3],
    pub zaxis: [f64; 3],
    pub xaxis: [f64; 3],
    pub r1: f64,
    pub r2: f64,
    pub udeg: i32,
    pub vdeg: i32,
    pub nu: i32,
    pub nv: i32,
    pub cp: *const f64,
    pub weights: *const f64,
    pub nuk: i32,
    pub nvk: i32,
    pub uknots: *const f64,
    pub vknots: *const f64,
    pub umults: *const i32,
    pub vmults: *const i32,
    pub profile: *const PwCurve,
    pub dir: [f64; 3],
}

unsafe extern "C" {
    pub fn pw_init();
    pub fn pw_free(brep: BrepPtr);
    pub fn pw_copy(brep: BrepPtr) -> BrepPtr;
    pub fn pw_box(sx: f64, sy: f64, sz: f64, out: *mut BrepPtr) -> i32;
    pub fn pw_cylinder(radius: f64, height: f64, out: *mut BrepPtr) -> i32;
    pub fn pw_extrude(points: *const f64, n: i32, dir: *const f64, out: *mut BrepPtr) -> i32;
    pub fn pw_transform(brep: BrepPtr, matrix: *const f64) -> i32;
    pub fn pw_scale(brep: BrepPtr, sx: f64, sy: f64, sz: f64) -> i32;
    pub fn pw_boolean(a: BrepPtr, b: BrepPtr, op: i32, out: *mut BrepPtr) -> i32;
    pub fn pw_is_solid(brep: BrepPtr) -> i32;
    pub fn pw_bounding_box(brep: BrepPtr, out: *mut f64) -> i32;
    pub fn pw_face_count(brep: BrepPtr) -> i32;
    pub fn pw_write_file(brep: BrepPtr, path: *const c_char) -> i32;
    pub fn pw_read_file(path: *const c_char, out: *mut BrepPtr) -> i32;

    pub fn pw_builder_new() -> BrepPtr;
    pub fn pw_builder_add_face(
        brep: BrepPtr,
        surface: *const PwSurface,
        same_sense: i32,
        loop_counts: *const i32,
        nloops: i32,
        curves: *const PwCurve,
        ncurves: i32,
        tolerance: f64,
    ) -> i32;
    pub fn pw_builder_finish(brep: BrepPtr, solid: *mut i32) -> i32;

    pub fn pw_tessellate(brep: BrepPtr, chord: f64, angle: f64) -> MeshPtr;
    pub fn pw_planar_polygon(points: *const f64, counts: *const i32, nloops: i32) -> MeshPtr;
    pub fn pw_mesh_face_count(mesh: MeshPtr) -> i32;
    pub fn pw_mesh_face(
        mesh: MeshPtr,
        i: i32,
        positions: *mut *const f64,
        normals: *mut *const f64,
        nvertices: *mut i32,
        triangles: *mut *const u32,
        ntriangles: *mut i32,
    );
    pub fn pw_mesh_face_loops(
        mesh: MeshPtr,
        i: i32,
        counts: *mut *const u32,
        nloops: *mut i32,
        edges: *mut *const u32,
        nedges: *mut i32,
    );
    pub fn pw_mesh_edge_count(mesh: MeshPtr) -> i32;
    pub fn pw_mesh_edge(mesh: MeshPtr, i: i32, points: *mut *const f64, npoints: *mut i32);
    pub fn pw_mesh_free(mesh: MeshPtr);

    // 形を作り変える操作と問い合わせ (cpp/ops.cpp)
    pub fn pw_fillet_edges(
        brep: BrepPtr,
        edges: *const u32,
        n: i32,
        size: f64,
        kind: i32,
        out: *mut BrepPtr,
    ) -> i32;
    pub fn pw_remove_fillets(brep: BrepPtr, out: *mut BrepPtr) -> i32;
    pub fn pw_shell(
        brep: BrepPtr,
        thickness: f64,
        faces: *const u32,
        n: i32,
        out: *mut BrepPtr,
    ) -> i32;
    pub fn pw_offset(brep: BrepPtr, distance: f64, out: *mut BrepPtr) -> i32;
    pub fn pw_revolve(
        points: *const f64,
        n: i32,
        base: *const f64,
        axis: *const f64,
        angle: f64,
        out: *mut BrepPtr,
    ) -> i32;
    pub fn pw_pipe(path: *const f64, n: i32, radius: f64, out: *mut BrepPtr) -> i32;
    pub fn pw_sweep(
        profile: *const f64,
        np: i32,
        path: *const f64,
        nq: i32,
        out: *mut BrepPtr,
    ) -> i32;
    pub fn pw_mass_properties(brep: BrepPtr, density: f64, out: *mut f64) -> i32;
    pub fn pw_section(
        brep: BrepPtr,
        origin: *const f64,
        normal: *const f64,
        samples: i32,
    ) -> LinesPtr;
    pub fn pw_lines_count(lines: LinesPtr) -> i32;
    pub fn pw_lines_get(lines: LinesPtr, points: *mut *const f64, counts: *mut *const u32);
    pub fn pw_lines_free(lines: LinesPtr);
    pub fn pw_heal(brep: BrepPtr) -> i32;
}
