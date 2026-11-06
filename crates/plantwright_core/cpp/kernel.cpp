// plantwright の形状カーネル (SMLib) を Rust から呼ぶための薄い層。
//
// Rust 側 (src/kernel/ffi.rs) と同じ形の C の関数・構造体だけを置く。SMLib の型は
// 外に出さず、B-rep は SmBrep* をそのまま不透明なポインタとして渡す。
// SMLib はプロセス全体で1つの状態を持ち、同時に複数のスレッドから呼べないので、
// 呼び出しは Rust 側でロックしてから行う。
//
// SPDX-License-Identifier: MIT OR Apache-2.0

#include <SmApiBrep.h>
#include <SmApiCurves.h>
#include <SmApiGeneral.h>
#include <SmApiImportExport.h>
#include <SmApiPrimitives.h>
#include <SmApiQueries.h>
#include <SmApiTessellationParams.h>
#include <SmApiTrimmedSurfaces.h>
#include <SmAxis2Placement.h>
#include <SmBSplineCurve.h>
#include <SmBSplineSurface.h>
#include <SmBrep.h>
#include <SmCircle.h>
#include <SmCone.h>
#include <SmContext.h>
#include <SmCylinder.h>
#include <SmDatabaseIO.h>
#include <SmEdge.h>
#include <SmEdgeuse.h>
#include <SmEllipse.h>
#include <SmFace.h>
#include <SmLine.h>
#include <SmPlane.h>
#include <SmPoly.h>
#include <SmRegion.h>
#include <SmSphere.h>
#include <SmSurfOfExtrusion.h>
#include <SmSurfOfRevolution.h>
#include <SmTorus.h>
#include <SmTrimmingTools.h>

#include <cmath>
#include <cstdint>
#include <cstring>
#include <map>
#include <sstream>
#include <string>
#include <vector>

namespace {

SmContext* context() { return SmApiGetOrCreateContext(); }

SmPoint3d point(const double* p) { return SmPoint3d(p[0], p[1], p[2]); }
SmVector3d vector(const double* v) { return SmVector3d(v[0], v[1], v[2]); }

SmVector3d cross(const SmVector3d& a, const SmVector3d& b) {
    return SmVector3d(a.y * b.z - a.z * b.y, a.z * b.x - a.x * b.z, a.x * b.y - a.y * b.x);
}

// 原点・Z 軸・X 軸から座標系を作る (Y = Z x X)
SmAxis2Placement placement(const double* origin, const double* zaxis, const double* xaxis) {
    SmVector3d z = vector(zaxis);
    SmVector3d x = vector(xaxis);
    z.Unitize(FALSE);
    // X を Z に垂直にそろえる
    double d = x.x * z.x + x.y * z.y + x.z * z.z;
    x = SmVector3d(x.x - z.x * d, x.y - z.y * d, x.z - z.z * d);
    x.Unitize(FALSE);
    return SmAxis2Placement(point(origin), x, cross(z, x));
}

bool ok(SmApiStatus status) { return status == SM_SUCCESS; }

}  // namespace

extern "C" {

// ---------------------------------------------------------------------------
// 曲線・曲面の記述 (Rust の kernel::ffi と同じ並び)
// ---------------------------------------------------------------------------

struct PwCurve {
    int32_t kind;  // 0: 線分, 1: 円弧, 2: 楕円弧, 3: B スプライン
    double p0[3];  // 線分・B スプライン: 輪郭の向きでの始点
    double p1[3];  // 線分・B スプライン: 輪郭の向きでの終点
    double center[3];
    double xaxis[3];
    double yaxis[3];
    double r1;  // 円の半径・楕円の X 方向の半径
    double r2;  // 楕円の Y 方向の半径
    double a0;  // 円弧・楕円弧の始まりの角度 (度, a0 < a1)
    double a1;
    int32_t degree;
    int32_t ncp;
    const double* cp;       // 制御点 xyz * ncp
    const double* weights;  // 重み (ncp 個) か NULL
    int32_t nknots;
    const double* knots;  // 重なりを除いた節点
    const int32_t* mults;
    int32_t full;     // B スプライン: 曲線全体を使う (閉じた辺)
    int32_t reverse;  // B スプライン: 輪郭の向きが曲線のパラメータと逆
};

struct PwSurface {
    int32_t kind;  // 0: 平面, 1: 円柱, 2: 円錐, 3: 球, 4: トーラス, 5: B スプライン, 6: 回転面, 7: 押し出し面
    double origin[3];
    double zaxis[3];
    double xaxis[3];
    double r1;  // 半径 (円錐は位置での半径・トーラスは大きい半径)
    double r2;  // 円錐の半頂角 (度)・トーラスの小さい半径
    int32_t udeg;
    int32_t vdeg;
    int32_t nu;
    int32_t nv;
    const double* cp;       // 制御点 xyz、u ごとに v を並べる (STEP と同じ)
    const double* weights;  // 重み (nu * nv) か NULL
    int32_t nuk;
    int32_t nvk;
    const double* uknots;
    const double* vknots;
    const int32_t* umults;
    const int32_t* vmults;
    const PwCurve* profile;  // 回転面・押し出し面の母線 (曲線全体)
    double dir[3];           // 押し出しの向きと長さ
};

}  // extern "C"

namespace {

SmBSplineCurve* bspline_curve(const PwCurve& c) {
    SmTArray<SmPoint3d> points;
    for (int32_t i = 0; i < c.ncp; i++) points.Add(point(c.cp + 3 * i));
    SmTArray<double> knots;
    SmTArray<ULONG> mults;
    for (int32_t i = 0; i < c.nknots; i++) {
        knots.Add(c.knots[i]);
        mults.Add((ULONG)c.mults[i]);
    }
    SmTArray<double> weights;
    if (c.weights) {
        for (int32_t i = 0; i < c.ncp; i++) weights.Add(c.weights[i]);
    }
    SmBSplineCurve* curve = nullptr;
    if (!ok(SmApiCreateCanonicalCurve(points, knots, mults, c.weights ? &weights : nullptr,
                                      (ULONG)c.degree, SM_CF_UNSPECIFIED, curve))) {
        return nullptr;
    }
    return curve;
}

double closest_param(const SmCurve* curve, const double* p) {
    SmPoint3d closest;
    double t = 0.0, distance = 0.0;
    if (!ok(SmApiCurveClosestPoint(curve, point(p), closest, t, distance))) return NAN;
    return t;
}

// 曲線の [t0, t1] の部分を新しい曲線として切り出す
SmCurve* sub_curve(const SmCurve* curve, double t0, double t1) {
    SmCurve* copy = nullptr;
    if (curve->Copy(*context(), copy) != SM_SUCCESS || !copy) return nullptr;
    SmExtent1d range(t0, t1);
    if (copy->Trim(range) != SM_SUCCESS) {
        delete copy;
        return nullptr;
    }
    return copy;
}

void reverse(SmCurve* curve) {
    SmExtent1d range = curve->GetNaturalInterval();
    SmExtent1d out;
    curve->ReverseParameterization(range, out);
}

// 辺の曲線 (輪郭の向き) を作る。B スプラインは周期の切れ目をまたぐと2本になる
bool edge_curves(const PwCurve& c, std::vector<SmCurve*>& out) {
    SmContext& ctx = *context();
    switch (c.kind) {
        case 0: {
            SmLine* line = nullptr;
            if (!ok(SmApiCreateLineSegment(point(c.p0), point(c.p1), line)) || !line) return false;
            out.push_back(line);
            return true;
        }
        case 1: {
            SmCircle* circle = new (ctx) SmCircle(point(c.center), vector(c.xaxis), vector(c.yaxis),
                                                  SmExtent1d(c.a0, c.a1), c.r1, 3, &ctx);
            out.push_back(circle);
            return true;
        }
        case 2: {
            SmEllipse* ellipse =
                new (ctx) SmEllipse(point(c.center), vector(c.xaxis), vector(c.yaxis),
                                    SmExtent1d(c.a0, c.a1), c.r1, c.r2, 3, &ctx);
            out.push_back(ellipse);
            return true;
        }
        case 3: {
            SmBSplineCurve* curve = bspline_curve(c);
            if (!curve) return false;
            SmExtent1d whole = curve->GetNaturalInterval();
            if (c.full) {
                if (c.reverse) reverse(curve);
                out.push_back(curve);
                return true;
            }
            // 輪郭の始点・終点を曲線のパラメータに直す
            double ts = closest_param(curve, c.p0);
            double te = closest_param(curve, c.p1);
            if (std::isnan(ts) || std::isnan(te)) {
                delete curve;
                return false;
            }
            double lo = c.reverse ? te : ts;
            double hi = c.reverse ? ts : te;
            double span = whole.GetMax() - whole.GetMin();
            double eps = span * 1e-9;
            std::vector<SmCurve*> pieces;
            if (lo < hi - eps) {
                if (lo <= whole.GetMin() + eps && hi >= whole.GetMax() - eps) {
                    pieces.push_back(curve);
                    curve = nullptr;
                } else {
                    SmCurve* piece = sub_curve(curve, lo, hi);
                    if (piece) pieces.push_back(piece);
                }
            } else {
                // 閉じた曲線の切れ目をまたぐ: [lo, 終わり] と [始め, hi] の2本
                if (lo < whole.GetMax() - eps) {
                    SmCurve* a = sub_curve(curve, lo, whole.GetMax());
                    if (a) pieces.push_back(a);
                }
                if (hi > whole.GetMin() + eps) {
                    SmCurve* b = sub_curve(curve, whole.GetMin(), hi);
                    if (b) pieces.push_back(b);
                }
            }
            delete curve;
            if (pieces.empty()) return false;
            if (c.reverse) {
                for (SmCurve* piece : pieces) reverse(piece);
                for (auto it = pieces.rbegin(); it != pieces.rend(); ++it) out.push_back(*it);
            } else {
                for (SmCurve* piece : pieces) out.push_back(piece);
            }
            return true;
        }
    }
    return false;
}

// 母線 (回転面・押し出し面) は曲線全体を B スプラインにする
SmBSplineCurve* profile_curve(const PwCurve& c) {
    SmContext& ctx = *context();
    switch (c.kind) {
        case 0: {
            SmLine* line = nullptr;
            if (!ok(SmApiCreateLineSegment(point(c.p0), point(c.p1), line))) return nullptr;
            return line;
        }
        case 1:
            return new (ctx) SmCircle(point(c.center), vector(c.xaxis), vector(c.yaxis),
                                      SmExtent1d(c.a0, c.a1), c.r1, 3, &ctx);
        case 2:
            return new (ctx) SmEllipse(point(c.center), vector(c.xaxis), vector(c.yaxis),
                                       SmExtent1d(c.a0, c.a1), c.r1, c.r2, 3, &ctx);
        case 3:
            return bspline_curve(c);
    }
    return nullptr;
}

SmSurface* make_surface(const PwSurface& s) {
    SmContext& ctx = *context();
    SmAxis2Placement frame = placement(s.origin, s.zaxis, s.xaxis);
    switch (s.kind) {
        case 0: {
            SmPlane* plane = nullptr;
            SmPlane::CreateCanonical(ctx, frame, plane);
            return plane;
        }
        case 1: {
            SmCylinder* cylinder = nullptr;
            SmCylinder::CreateCanonical(ctx, frame, s.r1, cylinder);
            return cylinder;
        }
        case 2: {
            SmCone* cone = nullptr;
            SmCone::CreateCanonical(ctx, frame, s.r1, s.r2, cone);
            return cone;
        }
        case 3: {
            SmSphere* sphere = nullptr;
            SmSphere::CreateCanonical(ctx, frame, s.r1, sphere);
            return sphere;
        }
        case 4: {
            SmTorus* torus = nullptr;
            SmTorus::CreateCanonical(ctx, frame, s.r1, s.r2, torus);
            return torus;
        }
        case 5: {
            SmTArray<SmPoint3d> points;
            for (int32_t i = 0; i < s.nu * s.nv; i++) points.Add(point(s.cp + 3 * i));
            SmTArray<ULONG> um, vm;
            SmTArray<double> uk, vk;
            for (int32_t i = 0; i < s.nuk; i++) {
                uk.Add(s.uknots[i]);
                um.Add((ULONG)s.umults[i]);
            }
            for (int32_t i = 0; i < s.nvk; i++) {
                vk.Add(s.vknots[i]);
                vm.Add((ULONG)s.vmults[i]);
            }
            SmTArray<double> weights;
            if (s.weights) {
                for (int32_t i = 0; i < s.nu * s.nv; i++) weights.Add(s.weights[i]);
            }
            SmBSplineSurface* surface = nullptr;
            SmBSplineSurface::CreateCanonical(ctx, (ULONG)s.udeg, (ULONG)s.vdeg, points,
                                              SM_SF_UNSPECIFIED, um, vm, uk, vk, SM_KT_UNSPECIFIED,
                                              s.weights ? &weights : nullptr, nullptr, surface);
            return surface;
        }
        case 6: {
            if (!s.profile) return nullptr;
            SmBSplineCurve* curve = profile_curve(*s.profile);
            if (!curve) return nullptr;
            SmSurfOfRevolution* surface = nullptr;
            SmSurfOfRevolution::CreateCanonical(ctx, curve, point(s.origin), vector(s.zaxis),
                                                surface);
            delete curve;
            return surface;
        }
        case 7: {
            if (!s.profile) return nullptr;
            SmBSplineCurve* curve = profile_curve(*s.profile);
            if (!curve) return nullptr;
            SmSurfOfExtrusion* surface = nullptr;
            SmSurfOfExtrusion::CreateCanonical(ctx, curve, vector(s.dir), surface);
            delete curve;
            return surface;
        }
    }
    return nullptr;
}

// ---------------------------------------------------------------------------
// テッセレーションの結果
// ---------------------------------------------------------------------------

struct FaceMesh {
    std::vector<double> positions;
    std::vector<double> normals;
    std::vector<uint32_t> triangles;
    std::vector<uint32_t> loop_counts;
    std::vector<uint32_t> loop_edges;
};

}  // namespace

struct PwMesh {
    std::vector<FaceMesh> faces;
    std::vector<std::vector<double>> edges;
};

namespace {

// B-rep の面ごとのメッシュ・辺ごとのポリライン・面の輪郭 (辺の番号) を作る
PwMesh* mesh_brep(SmBrep* brep, const SmTessellationParams& params) {
    SmTArray<SmFace*> faces;
    SmTArray<SmEdge*> edges;
    SmApiGetFaces(brep, faces);
    SmApiGetEdges(brep, edges);
    std::map<SmFace*, size_t> face_index;
    std::map<SmEdge*, size_t> edge_index;
    for (ULONG i = 0; i < faces.GetSize(); i++) face_index[faces[i]] = i;
    for (ULONG i = 0; i < edges.GetSize(); i++) edge_index[edges[i]] = i;

    PwMesh* mesh = new PwMesh();
    mesh->faces.resize(faces.GetSize());
    mesh->edges.resize(edges.GetSize());

    for (ULONG i = 0; i < faces.GetSize(); i++) {
        SmTArray<ULONG> counts;
        SmTArray<SmEdge*> loop_edges;
        SmTArray<SmOrientType> orientations;
        if (!ok(SmApiFaceGetBoundaryLoops(faces[i], counts, loop_edges, orientations))) continue;
        FaceMesh& face = mesh->faces[i];
        for (ULONG j = 0; j < counts.GetSize(); j++) face.loop_counts.push_back((uint32_t)counts[j]);
        for (ULONG j = 0; j < loop_edges.GetSize(); j++) {
            auto it = edge_index.find(loop_edges[j]);
            face.loop_edges.push_back(it == edge_index.end() ? UINT32_MAX : (uint32_t)it->second);
        }
    }

    SmPolyBrep* poly = nullptr;
    SmTArray<SmTessellationFailure> failures;
    if (!ok(SmApiTessellate(brep, poly, params, TRUE, &failures)) || !poly) {
        return mesh;
    }

    // 面ごとに、使う頂点に番号を振り直す
    std::vector<std::map<SmPolyVertex*, uint32_t>> local(faces.GetSize());
    SmTArray<SmPolyFace*> poly_faces;
    poly->GetPolyFaces(poly_faces);
    for (ULONG i = 0; i < poly_faces.GetSize(); i++) {
        SmPolyFace* pf = poly_faces[i];
        auto it = face_index.find(pf->GetOriginalFace());
        SmPolyLoop* loop = pf->GetOuterPolyLoop();
        if (it == face_index.end() || !loop) continue;
        FaceMesh& face = mesh->faces[it->second];
        SmTArray<SmPolyEdge*> pe;
        loop->GetPolyEdges(pe);
        std::vector<uint32_t> ids;
        for (ULONG j = 0; j < pe.GetSize(); j++) {
            SmPolyVertex* v = pe[j]->GetStartPolyVertex();
            auto& map = local[it->second];
            auto found = map.find(v);
            uint32_t id;
            if (found == map.end()) {
                id = (uint32_t)(face.positions.size() / 3);
                map[v] = id;
                const SmPoint3d& p = v->GetPoint();
                face.positions.insert(face.positions.end(), {p.x, p.y, p.z});
                // この面の曲面での法線
                SmVector3d n(0, 0, 0);
                const SmTArray<SmTopology*>& vf = v->GetFacesRef();
                const SmTArray<SmVector3d>& vn = v->GetNormalsRef();
                for (ULONG k = 0; k < vf.GetSize() && k < vn.GetSize(); k++) {
                    if (vf[k] == (SmTopology*)pf->GetOriginalFace()) {
                        n = vn[k];
                        break;
                    }
                }
                face.normals.insert(face.normals.end(), {n.x, n.y, n.z});
            } else {
                id = found->second;
            }
            ids.push_back(id);
        }
        // 多角形は扇形に三角形へ分ける (テッセレーションの結果はふつう三角形)
        for (size_t j = 1; j + 1 < ids.size(); j++) {
            face.triangles.insert(face.triangles.end(), {ids[0], ids[j], ids[j + 1]});
        }
    }

    // 辺のポリライン: 辺の1つの使われ方 (edgeuse) に沿った PolyEdge をつなぐ
    std::map<SmEdge*, SmEdgeuse*> chosen;
    std::map<SmEdge*, std::map<SmPolyVertex*, SmPolyVertex*>> next;
    std::map<SmEdge*, std::map<SmPolyVertex*, int>> incoming;
    SmTArray<SmPolyEdge*> poly_edges;
    for (ULONG i = 0; i < poly_faces.GetSize(); i++) {
        SmPolyLoop* loop = poly_faces[i]->GetOuterPolyLoop();
        if (!loop) continue;
        SmTArray<SmPolyEdge*> pe;
        loop->GetPolyEdges(pe);
        for (ULONG j = 0; j < pe.GetSize(); j++) {
            SmEdgeuse* eu = pe[j]->GetOriginalEdgeuse();
            if (!eu) continue;
            SmEdge* edge = eu->GetEdge();
            auto c = chosen.find(edge);
            if (c == chosen.end()) {
                chosen[edge] = eu;
            } else if (c->second != eu) {
                continue;
            }
            SmPolyVertex* a = pe[j]->GetStartPolyVertex();
            SmPolyVertex* b = pe[(j + 1) % pe.GetSize()]->GetStartPolyVertex();
            next[edge][a] = b;
            incoming[edge][b] += 1;
        }
    }
    for (auto& [edge, links] : next) {
        auto idx = edge_index.find(edge);
        if (idx == edge_index.end()) continue;
        // 入ってくる線のない頂点から始める (閉じた辺はどこからでもよい)
        SmPolyVertex* start = links.begin()->first;
        for (auto& [a, b] : links) {
            if (incoming[edge].find(a) == incoming[edge].end()) {
                start = a;
                break;
            }
        }
        std::vector<double>& line = mesh->edges[idx->second];
        SmPolyVertex* v = start;
        for (size_t guard = 0; guard <= links.size(); guard++) {
            const SmPoint3d& p = v->GetPoint();
            line.insert(line.end(), {p.x, p.y, p.z});
            auto n = links.find(v);
            if (n == links.end()) break;
            v = n->second;
            if (v == start) {
                const SmPoint3d& q = v->GetPoint();
                line.insert(line.end(), {q.x, q.y, q.z});
                break;
            }
        }
    }
    // 作ったばかりの基本形状などでは PolyEdge に元の辺が付かないことがある。
    // そのときは辺を同じ角度で標本化し、隣の面のメッシュの頂点にそろえる
    bool missing = false;
    for (auto& line : mesh->edges) missing = missing || line.empty();
    if (missing) {
        SmTArray<SmPoint3d> points;
        SmTArray<ULONG> counts;
        SmTArray<SmEdge*> sampled;
        SmApiTessellateBoundaries(brep, params.dCurveAngleTolDeg, points, counts, sampled);
        // 辺 → それを輪郭に持つ面
        std::vector<std::vector<size_t>> edge_faces(edges.GetSize());
        for (size_t f = 0; f < mesh->faces.size(); f++) {
            for (uint32_t e : mesh->faces[f].loop_edges) {
                if (e < edge_faces.size()) edge_faces[e].push_back(f);
            }
        }
        double snap = params.dChordHeightTol > 0 ? params.dChordHeightTol : 1e-6;
        ULONG base = 0;
        for (ULONG k = 0; k < sampled.GetSize() && k < counts.GetSize(); k++) {
            ULONG n = counts[k];
            auto idx = edge_index.find(sampled[k]);
            if (idx != edge_index.end() && mesh->edges[idx->second].empty() && n >= 2) {
                std::vector<double>& line = mesh->edges[idx->second];
                for (ULONG j = 0; j < n; j++) {
                    SmPoint3d p = points[base + j];
                    double best = snap * snap;
                    for (size_t f : edge_faces[idx->second]) {
                        const std::vector<double>& pos = mesh->faces[f].positions;
                        for (size_t q = 0; q + 2 < pos.size(); q += 3) {
                            double dx = pos[q] - p.x, dy = pos[q + 1] - p.y, dz = pos[q + 2] - p.z;
                            double d2 = dx * dx + dy * dy + dz * dz;
                            if (d2 <= best) {
                                best = d2;
                                p = SmPoint3d(pos[q], pos[q + 1], pos[q + 2]);
                            }
                        }
                    }
                    line.insert(line.end(), {p.x, p.y, p.z});
                }
            }
            base += n;
        }
    }
    delete poly;
    return mesh;
}

}  // namespace

extern "C" {

void pw_init() { SmApiCreateContext(); }

void pw_free(SmBrep* brep) { delete brep; }

SmBrep* pw_copy(const SmBrep* brep) {
    SmBrep* out = nullptr;
    if (!ok(SmApiBrepCopy(brep, out))) return nullptr;
    return out;
}

// 原点を最小の角にした直方体
int32_t pw_box(double sx, double sy, double sz, SmBrep** out) {
    *out = nullptr;
    return (int32_t)SmApiCreateBox(SmVector3d(0, 0, 0), sx, sy, sz, *out);
}

// 底面の中心を原点に置いた、+Z 方向の円柱
int32_t pw_cylinder(double radius, double height, SmBrep** out) {
    *out = nullptr;
    return (int32_t)SmApiCreateCylinder(SmVector3d(0, 0, 0), radius, height, *out);
}

// 閉じた多角形 (xyz * n) を dir の向きに押し出す (両端にふたをする)
int32_t pw_extrude(const double* points, int32_t n, const double* dir, SmBrep** out) {
    *out = nullptr;
    SmTArray<SmCurve*> curves;
    for (int32_t i = 0; i < n; i++) {
        SmLine* line = nullptr;
        SmApiStatus s =
            SmApiCreateLineSegment(point(points + 3 * i), point(points + 3 * ((i + 1) % n)), line);
        if (!ok(s)) {
            for (ULONG j = 0; j < curves.GetSize(); j++) delete curves[j];
            return (int32_t)s;
        }
        curves.Add(line);
    }
    SmVector3d d = vector(dir);
    double length = std::sqrt(d.x * d.x + d.y * d.y + d.z * d.z);
    d.Unitize(FALSE);
    SmApiStatus s = SmApiCreateLinearSweep(curves, d, length, TRUE, *out);
    for (ULONG j = 0; j < curves.GetSize(); j++) delete curves[j];
    return (int32_t)s;
}

// 回転と平行移動 (列優先の 4x4)
int32_t pw_transform(SmBrep* brep, const double* m) {
    double matrix[16];
    std::memcpy(matrix, m, sizeof(matrix));
    SmAxis2Placement frame;
    frame.SetFrom4x4(matrix);
    return (int32_t)SmApiTransform(brep, frame);
}

int32_t pw_scale(SmBrep* brep, double sx, double sy, double sz) {
    return (int32_t)SmApiScale(brep, SmVector3d(sx, sy, sz));
}

// a と b のブーリアン (0: 和, 1: 差, 2: 積)。a と b は変えずに新しい形を返す
int32_t pw_boolean(const SmBrep* a, const SmBrep* b, int32_t op, SmBrep** out) {
    *out = nullptr;
    SmBrep* ca = pw_copy(a);
    SmBrep* cb = pw_copy(b);
    if (!ca || !cb) {
        delete ca;
        delete cb;
        return (int32_t)SM_ERR;
    }
    SmBooleanOperationType type =
        op == 0 ? SM_BO_UNION : (op == 1 ? SM_BO_DIFFERENCE : SM_BO_INTERSECTION);
    SmBrep* result = nullptr;
    SmApiStatus s = SmApiBoolean(ca, cb, type, result);
    if (!ok(s) || !result) {
        delete ca;
        delete cb;
        return ok(s) ? (int32_t)SM_ERR : (int32_t)s;
    }
    *out = result;
    return (int32_t)SM_SUCCESS;
}

// 閉じたソリッドかどうか
int32_t pw_is_solid(const SmBrep* brep) {
    SmBoolean solid = FALSE;
    if (!ok(SmApiBrepIsManifoldSolid(brep, solid))) return 0;
    return solid ? 1 : 0;
}

int32_t pw_bounding_box(SmBrep* brep, double* out) {
    SmPoint3d lo, hi;
    SmApiStatus s = SmApiBrepBoundingBox(brep, FALSE, lo, hi);
    out[0] = lo.x;
    out[1] = lo.y;
    out[2] = lo.z;
    out[3] = hi.x;
    out[4] = hi.y;
    out[5] = hi.z;
    return (int32_t)s;
}

int32_t pw_face_count(SmBrep* brep) {
    SmTArray<SmFace*> faces;
    SmApiGetFaces(brep, faces);
    return (int32_t)faces.GetSize();
}

// ---------------------------------------------------------------------------
// 文字列への保存と読み込み (SMLib の .smb のテキスト形式)
// ---------------------------------------------------------------------------

// path はファイルのパス (UTF-8)。Rust 側が一時ファイルを用意して中身を読み書きする
int32_t pw_write_file(const SmBrep* brep, const char* path) {
    return (int32_t)SmApiWriteBrepToFile(brep, path, TRUE);
}

int32_t pw_read_file(const char* path, SmBrep** out) {
    *out = nullptr;
    return (int32_t)SmApiReadBrepFromFile(path, TRUE, FALSE, *out);
}

// ---------------------------------------------------------------------------
// 面を1つずつ足して B-rep を組み立てる (STEP などの読み込み)
// ---------------------------------------------------------------------------

SmBrep* pw_builder_new() { return new (*context()) SmBrep(); }

// 曲面 surface を輪郭 (外周が最初) で切り取った面を足す。same_sense が 0 なら面は曲面の法線と逆向き。
// curves は輪郭の向きに並べた辺 (loop_counts[i] 本ずつ)
int32_t pw_builder_add_face(SmBrep* brep, const PwSurface* surface, int32_t same_sense,
                            const int32_t* loop_counts, int32_t nloops, const PwCurve* curves,
                            int32_t ncurves, double tolerance) {
    SmTArray<ULONG> counts;
    SmTArray<SmCurve*> list;
    SmTArray<SmOrientType> orientations;
    int32_t next = 0;
    for (int32_t i = 0; i < nloops; i++) {
        ULONG count = 0;
        for (int32_t j = 0; j < loop_counts[i] && next < ncurves; j++, next++) {
            std::vector<SmCurve*> pieces;
            if (!edge_curves(curves[next], pieces)) continue;
            for (SmCurve* c : pieces) {
                list.Add(c);
                orientations.Add(SM_OT_SAME);
                count++;
            }
        }
        if (count > 0) counts.Add(count);
    }
    if (counts.GetSize() == 0) return (int32_t)SM_ERR_INVALID_INPUT;
    SmSurface* s = make_surface(*surface);
    if (!s) {
        for (ULONG j = 0; j < list.GetSize(); j++) delete list[j];
        return (int32_t)SM_ERR;
    }
    SmTArray<SmPoint3d> loop_points;
    SmTArray<SmFace*> faces;
    SmStatus status = SmTrimmingTools::TrimSurfaceWithModelSpaceCurves(
        *context(), brep->GetInfiniteRegion(), tolerance, counts, list, orientations, loop_points,
        s, s->GetNaturalUVDomain(), same_sense ? SM_OT_SAME : SM_OT_OPPOSITE, TRUE, TRUE, TRUE,
        &faces);
    return (int32_t)status;
}

// 足した面を縫い合わせる。閉じたソリッドになれば内側を中身にする。solid に結果を入れる
int32_t pw_builder_finish(SmBrep* brep, int32_t* solid) {
    *solid = 0;
    SmBoolean is_solid = FALSE;
    ULONG stitched = 0;
    double vertex_gap = 0.0, edge_gap = 0.0;
    SmApiStatus s = SmApiStitchIntoSolid(brep, is_solid, stitched, vertex_gap, edge_gap);
    if (!ok(s)) return (int32_t)s;
    SmBoolean manifold = FALSE;
    SmApiBrepIsManifoldSolid(brep, manifold);
    if (manifold) {
        brep->FindAndSetInfiniteRegion(NULL, SM_VS_NESTEDSOLID);
        *solid = 1;
    }
    return (int32_t)SM_SUCCESS;
}

// ---------------------------------------------------------------------------
// テッセレーション
// ---------------------------------------------------------------------------

// chord: 曲面からのずれの上限 (0 で使わない)、angle: 曲面・曲線の向きの変わり方の上限 (度)
PwMesh* pw_tessellate(SmBrep* brep, double chord, double angle) {
    SmTessellationParams params;
    params.dChordHeightTol = chord;
    params.dCurveAngleTolDeg = angle;
    params.dSurfaceAngleTolDeg = angle;
    return mesh_brep(brep, params);
}

// 平面の多角形 (輪郭ごとの点の列、最初が外周) を三角形にする
PwMesh* pw_planar_polygon(const double* points, const int32_t* counts, int32_t nloops) {
    SmTArray<SmCurve*> curves;
    int32_t base = 0;
    for (int32_t i = 0; i < nloops; i++) {
        int32_t n = counts[i];
        for (int32_t j = 0; j < n; j++) {
            const double* a = points + 3 * (base + j);
            const double* b = points + 3 * (base + (j + 1) % n);
            if (std::fabs(a[0] - b[0]) + std::fabs(a[1] - b[1]) + std::fabs(a[2] - b[2]) <= 0.0) {
                continue;
            }
            SmLine* line = nullptr;
            if (ok(SmApiCreateLineSegment(point(a), point(b), line)) && line) curves.Add(line);
        }
        base += n;
    }
    SmBrep* brep = nullptr;
    if (!ok(SmApiCreatePlanarFaces(curves, brep)) || !brep) {
        for (ULONG j = 0; j < curves.GetSize(); j++) delete curves[j];
        return nullptr;
    }
    SmTessellationParams params;
    PwMesh* mesh = mesh_brep(brep, params);
    delete brep;
    return mesh;
}

int32_t pw_mesh_face_count(const PwMesh* m) { return (int32_t)m->faces.size(); }

void pw_mesh_face(const PwMesh* m, int32_t i, const double** positions, const double** normals,
                  int32_t* nvertices, const uint32_t** triangles, int32_t* ntriangles) {
    const FaceMesh& f = m->faces[i];
    *positions = f.positions.data();
    *normals = f.normals.data();
    *nvertices = (int32_t)(f.positions.size() / 3);
    *triangles = f.triangles.data();
    *ntriangles = (int32_t)(f.triangles.size() / 3);
}

void pw_mesh_face_loops(const PwMesh* m, int32_t i, const uint32_t** counts, int32_t* nloops,
                        const uint32_t** edges, int32_t* nedges) {
    const FaceMesh& f = m->faces[i];
    *counts = f.loop_counts.data();
    *nloops = (int32_t)f.loop_counts.size();
    *edges = f.loop_edges.data();
    *nedges = (int32_t)f.loop_edges.size();
}

int32_t pw_mesh_edge_count(const PwMesh* m) { return (int32_t)m->edges.size(); }

void pw_mesh_edge(const PwMesh* m, int32_t i, const double** points, int32_t* npoints) {
    *points = m->edges[i].data();
    *npoints = (int32_t)(m->edges[i].size() / 3);
}

void pw_mesh_free(PwMesh* m) { delete m; }

}  // extern "C"
