// 形を作り変える操作と問い合わせ (フィレット・面取り・シェル・オフセット・回転・パイプ・
// スイープ・質量特性・断面・修復)。Rust 側は src/kernel/ops.rs。
//
// どの操作も入力の形は変えず、複製に対して行って新しい形を返す。
// 辺と面の番号は SmApiGetEdges / SmApiGetFaces の並び (= テッセレーションの番号) と同じ。
//
// SPDX-License-Identifier: MIT OR Apache-2.0

#include <SmApiBrep.h>
#include <SmApiCurves.h>
#include <SmApiFillets.h>
#include <SmApiGeneral.h>
#include <SmApiHeal.h>
#include <SmApiIntersectors.h>
#include <SmApiQueries.h>
#include <SmBSplineCurve.h>
#include <SmBrep.h>
#include <SmCurve.h>
#include <SmEdge.h>
#include <SmFace.h>
#include <SmLine.h>

#include <cmath>
#include <cstdint>
#include <vector>

namespace {

SmPoint3d point(const double* p) { return SmPoint3d(p[0], p[1], p[2]); }
SmVector3d vector(const double* v) { return SmVector3d(v[0], v[1], v[2]); }
bool ok(SmApiStatus status) { return status == SM_SUCCESS; }

SmBrep* copy_of(const SmBrep* brep) {
    SmBrep* out = nullptr;
    if (!ok(SmApiBrepCopy(brep, out))) return nullptr;
    return out;
}

void free_curves(SmTArray<SmCurve*>& curves) {
    for (ULONG i = 0; i < curves.GetSize(); i++) delete curves[i];
    curves.RemoveAll();
}

// 点の列 (xyz * n) を線分でつなぐ。closed なら最後の点から最初の点へも結ぶ
bool polyline(const double* points, int32_t n, bool closed, SmTArray<SmCurve*>& out) {
    int32_t segments = closed ? n : n - 1;
    for (int32_t i = 0; i < segments; i++) {
        const double* a = points + 3 * i;
        const double* b = points + 3 * ((i + 1) % n);
        if (a[0] == b[0] && a[1] == b[1] && a[2] == b[2]) continue;
        SmLine* line = nullptr;
        if (!ok(SmApiCreateLineSegment(point(a), point(b), line)) || !line) {
            free_curves(out);
            return false;
        }
        out.Add(line);
    }
    return out.GetSize() > 0;
}

// 点を通る曲線 (4点以上はなめらかに補間、それより少なければ折れ線の1次 B スプライン)
SmBSplineCurve* path_curve(const double* points, int32_t n) {
    SmTArray<SmPoint3d> pts;
    for (int32_t i = 0; i < n; i++) pts.Add(point(points + 3 * i));
    if (n >= 4) {
        SmCurve* curve = nullptr;
        if (ok(SmApiCreateCurveInterpPoints(pts, curve, SM_CP_CHORDLENGTH)) && curve) {
            return static_cast<SmBSplineCurve*>(curve);
        }
    }
    SmTArray<double> knots;
    SmTArray<ULONG> mults;
    for (int32_t i = 0; i < n; i++) {
        knots.Add((double)i);
        mults.Add(i == 0 || i == n - 1 ? 2 : 1);
    }
    SmBSplineCurve* curve = nullptr;
    if (!ok(SmApiCreateCanonicalCurve(pts, knots, mults, nullptr, 1, SM_CF_POLYLINE_FORM, curve))) {
        return nullptr;
    }
    return curve;
}

}  // namespace

// 曲線をポリラインにしたもの (断面の結果)
struct PwLines {
    std::vector<double> points;
    std::vector<uint32_t> counts;
};

extern "C" {

// 辺 (番号) を丸める (kind 1) か面取りする (kind 0)。size は半径または面取りの幅
int32_t pw_fillet_edges(const SmBrep* brep, const uint32_t* edges, int32_t n, double size,
                        int32_t kind, SmBrep** out) {
    *out = nullptr;
    SmBrep* work = copy_of(brep);
    if (!work) return (int32_t)SM_ERR;
    SmTArray<SmEdge*> all;
    SmApiGetEdges(work, all);
    SmTArray<SmEdge*> chosen;
    for (int32_t i = 0; i < n; i++) {
        if (edges[i] < all.GetSize()) chosen.Add(all[edges[i]]);
    }
    if (chosen.GetSize() == 0) {
        delete work;
        return (int32_t)SM_ERR_INVALID_INPUT;
    }
    SmApiStatus s = SmApiFilletEdges(work, chosen, size, kind == 0 ? 0 : 1, 1, 1.0);
    if (!ok(s)) {
        delete work;
        return (int32_t)s;
    }
    *out = work;
    return (int32_t)SM_SUCCESS;
}

// 形にあるフィレットを見つけて消す
int32_t pw_remove_fillets(const SmBrep* brep, SmBrep** out) {
    *out = nullptr;
    SmBrep* work = copy_of(brep);
    if (!work) return (int32_t)SM_ERR;
    SmApiStatus s = SmApiRemoveFillet(work);
    if (!ok(s)) {
        delete work;
        return (int32_t)s;
    }
    *out = work;
    return (int32_t)SM_SUCCESS;
}

// 厚み thickness の中空にする。faces (番号) の面は開ける。thickness が正なら内側に肉を付ける
int32_t pw_shell(const SmBrep* brep, double thickness, const uint32_t* faces, int32_t n,
                 SmBrep** out) {
    *out = nullptr;
    SmTArray<SmFace*> all;
    SmApiGetFaces(const_cast<SmBrep*>(brep), all);
    SmTArray<const SmFace*> open;
    for (int32_t i = 0; i < n; i++) {
        if (faces[i] < all.GetSize()) open.Add(all[faces[i]]);
    }
    return (int32_t)SmApiShellBrepFull(brep, -thickness, TRUE, TRUE, TRUE, open, *out);
}

// 面を distance だけ外へ (負なら内へ) ずらした形
int32_t pw_offset(const SmBrep* brep, double distance, SmBrep** out) {
    *out = nullptr;
    SmBrep* work = copy_of(brep);
    if (!work) return (int32_t)SM_ERR;
    SmApiStatus s = SmApiOffsetBrepFull(work, distance, TRUE, TRUE, *out);
    delete work;
    return (int32_t)s;
}

// 閉じた多角形を軸のまわりに angle 度回した形 (360 度未満なら両端にふたをする)
int32_t pw_revolve(const double* points, int32_t n, const double* base, const double* axis,
                   double angle, SmBrep** out) {
    *out = nullptr;
    SmTArray<SmCurve*> curves;
    if (!polyline(points, n, true, curves)) return (int32_t)SM_ERR_INVALID_INPUT;
    SmApiStatus s = SmApiCreateRotationalSweep(curves, point(base), vector(axis), angle,
                                               angle < 360.0 ? TRUE : FALSE, *out);
    free_curves(curves);
    return (int32_t)s;
}

// 点の列を通る線に沿った、半径 radius の管 (両端にふた)
int32_t pw_pipe(const double* path, int32_t n, double radius, SmBrep** out) {
    *out = nullptr;
    if (n < 2) return (int32_t)SM_ERR_INVALID_INPUT;
    SmBSplineCurve* curve = path_curve(path, n);
    if (!curve) return (int32_t)SM_ERR;
    SmApiStatus s = SmApiCreatePipeSweep(radius, curve, TRUE, *out);
    delete curve;
    return (int32_t)s;
}

// 閉じた多角形 (断面) を、平面上の折れ線 path に沿って動かした形。断面の中心を path の始点に置く
int32_t pw_sweep(const double* profile, int32_t np, const double* path, int32_t nq, SmBrep** out) {
    *out = nullptr;
    SmTArray<SmCurve*> section;
    SmTArray<SmCurve*> route;
    if (!polyline(profile, np, true, section)) return (int32_t)SM_ERR_INVALID_INPUT;
    if (!polyline(path, nq, false, route)) {
        free_curves(section);
        return (int32_t)SM_ERR_INVALID_INPUT;
    }
    SmApiStatus s = SmApiCreateSweepAlongPlanarPath(section, route, TRUE, 3, *out);
    free_curves(section);
    free_curves(route);
    return (int32_t)s;
}

// 密度 density の質量特性。out は 面積, 体積, 質量, 重心 xyz,
// 重心まわりの慣性モーメント (Ixx, Iyy, Izz), 慣性乗積 (Iyz, Izx, Ixy) の 12 個
int32_t pw_mass_properties(const SmBrep* brep, double density, double* out) {
    double area = 0, volume = 0, mass = 0;
    SmPoint3d centroid;
    SmVector3d moments, products;
    SmApiStatus s = SmApiBrepComputeMassProperties(brep, 1e-6, density, SmPoint3d(0, 0, 0), area,
                                                   volume, mass, centroid, moments, products);
    if (!ok(s)) return (int32_t)s;
    // 重心のまわりで測り直す
    s = SmApiBrepComputeMassProperties(brep, 1e-6, density, centroid, area, volume, mass, centroid,
                                       moments, products);
    if (!ok(s)) return (int32_t)s;
    double values[12] = {area,      volume,    mass,       centroid.x, centroid.y, centroid.z,
                         moments.x, moments.y, moments.z,  products.x, products.y, products.z};
    for (int i = 0; i < 12; i++) out[i] = values[i];
    return (int32_t)SM_SUCCESS;
}

// 平面 (点と法線) で切った断面の線を、1本あたり samples 個の点にする
PwLines* pw_section(const SmBrep* brep, const double* origin, const double* normal,
                    int32_t samples) {
    SmBrep* work = copy_of(brep);
    if (!work) return nullptr;
    SmTArray<SmCurve*> curves;
    SmApiStatus s = SmApiIntersectBrepWithPlane(work, vector(origin), vector(normal), curves);
    delete work;
    PwLines* lines = new PwLines();
    if (!ok(s)) {
        free_curves(curves);
        return lines;
    }
    int32_t n = samples < 2 ? 2 : samples;
    for (ULONG i = 0; i < curves.GetSize(); i++) {
        SmCurve* curve = curves[i];
        SmExtent1d range = curve->GetNaturalInterval();
        uint32_t count = 0;
        // 直線は両端だけでよい
        int32_t steps = dynamic_cast<SmLine*>(curve) ? 1 : n - 1;
        for (int32_t k = 0; k <= steps; k++) {
            double t = range.GetMin() + (range.GetMax() - range.GetMin()) * k / steps;
            SmVector3d p;
            SmVector3d* pp = &p;
            SmVector3d* d1 = nullptr;
            SmVector3d* d2 = nullptr;
            if (!ok(SmApiEvaluateCurve(curve, t, pp, d1, d2))) continue;
            lines->points.insert(lines->points.end(), {p.x, p.y, p.z});
            count++;
        }
        if (count > 0) lines->counts.push_back(count);
    }
    free_curves(curves);
    return lines;
}

int32_t pw_lines_count(const PwLines* l) { return (int32_t)l->counts.size(); }

void pw_lines_get(const PwLines* l, const double** points, const uint32_t** counts) {
    *points = l->points.data();
    *counts = l->counts.data();
}

void pw_lines_free(PwLines* l) { delete l; }

// すき間や向きの乱れを直す (その場で書き換える)
int32_t pw_heal(SmBrep* brep) { return (int32_t)SmApiHealBrep(brep); }

}  // extern "C"
