"""サンプルの STEP アセンブリ (assets/sample_assembly.step) を作るスクリプト。

外部ライブラリは使わず、箱と円柱だけの B-rep を AP214 形式で書き出す。
構成:
    SampleVise
    ├── BasePlate          (箱)
    ├── Bracket_L ─┐       (サブアセンブリ Bracket の1つ目)
    ├── Bracket_R ─┴─ Bracket
    │                 ├── Upright  (箱)
    │                 └── Bushing  (円柱)
    ├── Shaft              (円柱)
    └── Knob               (円柱)

実行: python3 tools/make_sample_step.py
"""

from pathlib import Path


class StepWriter:
    def __init__(self):
        self.lines = []
        self.next_id = 1

    def add(self, text):
        entity_id = self.next_id
        self.next_id += 1
        self.lines.append(f"#{entity_id} = {text};")
        return f"#{entity_id}"

    # --- 幾何 ---------------------------------------------------------------
    def point(self, p):
        return self.add(f"CARTESIAN_POINT('',({fmt(p[0])},{fmt(p[1])},{fmt(p[2])}))")

    def direction(self, d):
        return self.add(f"DIRECTION('',({fmt(d[0])},{fmt(d[1])},{fmt(d[2])}))")

    def axis2(self, origin, axis=(0, 0, 1), ref=(1, 0, 0)):
        return self.add(
            f"AXIS2_PLACEMENT_3D('',{self.point(origin)},{self.direction(axis)},{self.direction(ref)})"
        )

    def vertex(self, p):
        return self.add(f"VERTEX_POINT('',{self.point(p)})")

    def line_edge(self, v0, v1, p0, p1):
        d = [b - a for a, b in zip(p0, p1)]
        length = sum(c * c for c in d) ** 0.5
        unit = [c / length for c in d]
        vec = self.add(f"VECTOR('',{self.direction(unit)},{fmt(length)})")
        line = self.add(f"LINE('',{self.point(p0)},{vec})")
        return self.add(f"EDGE_CURVE('',{v0},{v1},{line},.T.)")

    def face(self, oriented_edges, surface):
        loop = self.add(f"EDGE_LOOP('',({','.join(oriented_edges)}))")
        bound = self.add(f"FACE_OUTER_BOUND('',{loop},.T.)")
        return self.add(f"ADVANCED_FACE('',({bound}),{surface},.T.)")

    def oriented(self, edge, forward):
        return self.add(f"ORIENTED_EDGE('',*,*,{edge},{'.T.' if forward else '.F.'})")

    def solid(self, name, faces):
        shell = self.add(f"CLOSED_SHELL('',({','.join(faces)}))")
        return self.add(f"MANIFOLD_SOLID_BREP('{name}',{shell})")

    def box(self, name, size):
        """原点を角に、+X/+Y/+Z 方向へ size だけ伸びる箱"""
        sx, sy, sz = size
        corners = {
            (i, j, k): (i * sx, j * sy, k * sz)
            for i in (0, 1) for j in (0, 1) for k in (0, 1)
        }
        vertices = {key: self.vertex(p) for key, p in corners.items()}
        edges = {}

        def edge(a, b):
            if (b, a) in edges:
                return edges[(b, a)], False
            if (a, b) not in edges:
                edges[(a, b)] = self.line_edge(vertices[a], vertices[b], corners[a], corners[b])
            return edges[(a, b)], True

        # 外から見て反時計回りになる順で角を並べる
        quads = [
            ((0, 0, 0), (0, 1, 0), (1, 1, 0), (1, 0, 0), (0, 0, -1)),  # 底
            ((0, 0, 1), (1, 0, 1), (1, 1, 1), (0, 1, 1), (0, 0, 1)),  # 上
            ((0, 0, 0), (1, 0, 0), (1, 0, 1), (0, 0, 1), (0, -1, 0)),  # 前
            ((0, 1, 0), (0, 1, 1), (1, 1, 1), (1, 1, 0), (0, 1, 0)),  # 後
            ((0, 0, 0), (0, 0, 1), (0, 1, 1), (0, 1, 0), (-1, 0, 0)),  # 左
            ((1, 0, 0), (1, 1, 0), (1, 1, 1), (1, 0, 1), (1, 0, 0)),  # 右
        ]
        faces = []
        for *keys, normal in quads:
            oriented = []
            for a, b in zip(keys, keys[1:] + keys[:1]):
                e, forward = edge(a, b)
                oriented.append(self.oriented(e, forward))
            p0, p1 = corners[keys[0]], corners[keys[1]]
            ref = [b - a for a, b in zip(p0, p1)]
            ref = [c / max(abs(x) for x in ref) for c in ref]
            plane = self.add(f"PLANE('',{self.axis2(p0, normal, ref)})")
            faces.append(self.face(oriented, plane))
        return self.solid(name, faces)

    def cylinder(self, name, radius, height):
        """原点を底面の中心に、+Z 方向へ伸びる円柱"""
        v0 = self.vertex((radius, 0, 0))
        v1 = self.vertex((radius, 0, height))
        circles = []
        for z, v in ((0, v0), (height, v1)):
            circle = self.add(f"CIRCLE('',{self.axis2((0, 0, z))},{fmt(radius)})")
            circles.append(self.add(f"EDGE_CURVE('',{v},{v},{circle},.T.)"))
        seam = self.line_edge(v0, v1, (radius, 0, 0), (radius, 0, height))
        bottom = self.face(
            [self.oriented(circles[0], False)],
            self.add(f"PLANE('',{self.axis2((0, 0, 0), (0, 0, -1))})"),
        )
        top = self.face(
            [self.oriented(circles[1], True)],
            self.add(f"PLANE('',{self.axis2((0, 0, height))})"),
        )
        side = self.face(
            [
                self.oriented(circles[0], True),
                self.oriented(seam, True),
                self.oriented(circles[1], False),
                self.oriented(seam, False),
            ],
            self.add(f"CYLINDRICAL_SURFACE('',{self.axis2((0, 0, 0))},{fmt(radius)})"),
        )
        return self.solid(name, [bottom, top, side])


def fmt(x):
    """STEP の実数表記 (小数点が必須) にする"""
    return repr(float(x)).upper()


def main():
    w = StepWriter()
    app = w.add("APPLICATION_CONTEXT('core data for automotive mechanical design processes')")
    w.add(f"APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2000,{app})")
    mm = w.add("( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) )")
    rad = w.add("( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) )")
    sr = w.add("( NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT() )")
    unc = w.add(f"UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-07),{mm},'distance_accuracy_value','')")
    ctx = w.add(
        f"( GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT(({unc})) "
        f"GLOBAL_UNIT_ASSIGNED_CONTEXT(({mm},{rad},{sr})) REPRESENTATION_CONTEXT('',''))"
    )
    origin = w.axis2((0, 0, 0))

    def product(name):
        pc = w.add(f"PRODUCT_CONTEXT('',{app},'mechanical')")
        prod = w.add(f"PRODUCT('{name}','{name}','',({pc}))")
        pdf = w.add(f"PRODUCT_DEFINITION_FORMATION('','',{prod})")
        pdc = w.add(f"PRODUCT_DEFINITION_CONTEXT('part definition',{app},'design')")
        return w.add(f"PRODUCT_DEFINITION('design','',{pdf},{pdc})")

    def part(name, solid, rgb):
        pd = product(name)
        rep = w.add(f"ADVANCED_BREP_SHAPE_REPRESENTATION('',({origin},{solid}),{ctx})")
        pds = w.add(f"PRODUCT_DEFINITION_SHAPE('','',{pd})")
        w.add(f"SHAPE_DEFINITION_REPRESENTATION({pds},{rep})")
        color = w.add(f"COLOUR_RGB('',{fmt(rgb[0])},{fmt(rgb[1])},{fmt(rgb[2])})")
        fac = w.add(f"FILL_AREA_STYLE_COLOUR('',{color})")
        fill = w.add(f"FILL_AREA_STYLE('',({fac}))")
        ssfa = w.add(f"SURFACE_STYLE_FILL_AREA({fill})")
        sss = w.add(f"SURFACE_SIDE_STYLE('',({ssfa}))")
        ssu = w.add(f"SURFACE_STYLE_USAGE(.BOTH.,{sss})")
        psa = w.add(f"PRESENTATION_STYLE_ASSIGNMENT(({ssu}))")
        w.add(f"STYLED_ITEM('color',({psa}),{solid})")
        return pd, rep

    def assembly(name, placements):
        pd = product(name)
        rep = w.add(f"SHAPE_REPRESENTATION('',({origin},{','.join(placements)}),{ctx})")
        pds = w.add(f"PRODUCT_DEFINITION_SHAPE('','',{pd})")
        w.add(f"SHAPE_DEFINITION_REPRESENTATION({pds},{rep})")
        return pd, rep

    counter = [0]

    def instance(name, parent, child, placement):
        counter[0] += 1
        parent_pd, parent_rep = parent
        child_pd, child_rep = child
        nauo = w.add(
            f"NEXT_ASSEMBLY_USAGE_OCCURRENCE('{counter[0]}','{name}','',{parent_pd},{child_pd},$)"
        )
        pds = w.add(f"PRODUCT_DEFINITION_SHAPE('','',{nauo})")
        idt = w.add(f"ITEM_DEFINED_TRANSFORMATION('','',{origin},{placement})")
        rr = w.add(
            f"( REPRESENTATION_RELATIONSHIP('','',{child_rep},{parent_rep}) "
            f"REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION({idt}) SHAPE_REPRESENTATION_RELATIONSHIP() )"
        )
        w.add(f"CONTEXT_DEPENDENT_SHAPE_REPRESENTATION({rr},{pds})")

    x_axis = ((1, 0, 0), (0, 1, 0))  # 円柱の軸 (+Z) を +X に向ける配置

    # --- 部品 -----------------------------------------------------------------
    base = part("BasePlate", w.box("BasePlate", (240, 100, 20)), (0.55, 0.60, 0.68))
    upright = part("Upright", w.box("Upright", (20, 60, 90)), (0.40, 0.62, 0.48))
    bushing = part("Bushing", w.cylinder("Bushing", 14, 30), (0.85, 0.65, 0.20))
    shaft = part("Shaft", w.cylinder("Shaft", 8, 260), (0.80, 0.80, 0.82))
    knob = part("Knob", w.cylinder("Knob", 22, 16), (0.85, 0.25, 0.25))

    # --- サブアセンブリ Bracket (立て板 + ブッシュ) ----------------------------
    p_upright = w.axis2((-10, -30, 0))
    p_bushing = w.axis2((-15, 0, 70), *x_axis)
    bracket = assembly("Bracket", [p_upright, p_bushing])
    instance("Upright_1", bracket, upright, p_upright)
    instance("Bushing_1", bracket, bushing, p_bushing)

    # --- トップアセンブリ -------------------------------------------------------
    p_base = w.axis2((-120, -50, -20))
    p_left = w.axis2((-80, 0, 0))
    p_right = w.axis2((80, 0, 0))
    p_shaft = w.axis2((-130, 0, 70), *x_axis)
    p_knob = w.axis2((130, 0, 70), *x_axis)
    top = assembly("SampleVise", [p_base, p_left, p_right, p_shaft, p_knob])
    instance("BasePlate_1", top, base, p_base)
    instance("Bracket_L", top, bracket, p_left)
    instance("Bracket_R", top, bracket, p_right)
    instance("Shaft_1", top, shaft, p_shaft)
    instance("Knob_1", top, knob, p_knob)

    header = """ISO-10303-21;
HEADER;
FILE_DESCRIPTION(('cad_proto sample assembly'),'2;1');
FILE_NAME('sample_assembly.step','2026-10-30T00:00:00',(''),(''),'make_sample_step.py','cad_proto','');
FILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));
ENDSEC;
DATA;
"""
    out = Path(__file__).resolve().parent.parent / "assets" / "sample_assembly.step"
    out.write_text(header + "\n".join(w.lines) + "\nENDSEC;\nEND-ISO-10303-21;\n")
    print(f"wrote {out} ({w.next_id - 1} entities)")


if __name__ == "__main__":
    main()
