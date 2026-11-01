#!/usr/bin/env python3
"""本物の装置の CAD で組んだピック & プレースのセル (samples/real_robot/ur5e_pick_place.plantwright) を作る。

UR5e (Universal Robots) に Robotiq 2F-85 グリッパを付け、供給コンベアのワークを排出コンベアへ移します。
形状は同じフォルダの ur5e.glb / robotiq_2f85.glb を読み込み、このスクリプトは

- 読み込んだリンクを関節 (回転テーブル) にして、根元から順に「載せ先」でつなぐ
- グリッパの指のリンク機構を関節にする (ROS の記述と同じく、指先は外側の関節と逆に回して平行に保つ)
- ロボット・教示点・タイミングチャート・コンベア・センサーを置く

だけを書きます。関節の位置と向きは kinematics.py でメーカー公開の寸法から求めます。

    python3 tools/real_robot/make_demo.py            # samples/real_robot/ur5e_pick_place.plantwright に書く
    python3 tools/real_robot/make_demo.py out.plantwright

座標は mm、Y が上。ロボットは +X を向いて立ち、その前を 2 本のコンベアが +Z へ流れます。
関節の値は「ホーム姿勢 (kinematics.UR5E_HOME) からの角度」です。
"""

import json
import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import kinematics as k  # noqa: E402

M = 1000.0  # m → mm

# ---------------------------------------------------------------------------
# 配置 (mm)
# ---------------------------------------------------------------------------
PEDESTAL_H = 700.0  # ロボットの架台の高さ (ロボットの原点 = 架台の上面の中心)
BELT_TOP = 750.0
BELT_W = 300.0
LINE_X = 560.0  # コンベアの中心線
WORK = [60.0, 60.0, 60.0]  # ワーク (幅 / 高さ / 奥行)
# ワークの上のほうを掴む (閉じると指先が下がるので、ベルトやプーリーに当たらない高さにする)
GRASP_Y = BELT_TOP + WORK[1] - 22.0
IN_END = -230.0  # 供給コンベアの終端 (ストッパ) の Z
IN_LEN = 1000.0
OUT_START = 230.0
OUT_LEN = 1300.0
PICK = [LINE_X, GRASP_Y, IN_END - WORK[2] / 2]
PLACE = [LINE_X, GRASP_Y, 520.0]
LIFT = 150.0
TCP_FROM_FLANGE = 0.145  # tool0 から指先の中央 (掴む位置) まで (m)
IN_SPEED = 300.0
# ワークが投入されてからストッパに着くまで (s) + 余裕。最初のステップをこれだけ待たせる
FIRST_DWELL = round(IN_LEN / IN_SPEED + 0.5 - 1.0 - 0.6, 2)
GRIP_ANGLE = k.gripper_angle_for(WORK[0] / M - 0.002)  # ワークより 2 mm 狭く閉じる (rad)

BASE = [0.0, PEDESTAL_H, 0.0]

STEEL = [0.78, 0.80, 0.84]
DARK = [0.22, 0.24, 0.28]
BLUE = [0.25, 0.50, 0.85]
GREEN = [0.30, 0.70, 0.45]

# ---------------------------------------------------------------------------
# 座標の変換 (ROS の m・Z 上 → plantwright の mm・Y 上、ロボットの原点を BASE に)
# ---------------------------------------------------------------------------


def to_world(m_ros):
    """ROS の座標で書いた (ロボットの原点から見た) 座標系 → plantwright のワールドから見た座標系。
    局所の座標は ROS のまま (m の代わりに mm) で、写した先がワールド (mm・Y 上)"""
    m = k.chain(k.ZUP_TO_YUP, m_ros)
    for i in range(3):
        m[i][3] = m[i][3] * M + BASE[i]
    return m


def axis_letter(v):
    """座標軸に沿った向き → ("X" / "Y" / "Z", 逆向きか)"""
    i = max(range(3), key=lambda j: abs(v[j]))
    assert abs(abs(v[i]) - 1.0) < 1e-6, f"関節の軸が座標軸に沿っていません: {v}"
    return "XYZ"[i], v[i] < 0


def euler_deg(m):
    """回転行列 → plantwright の rotation_deg (Rz · Ry · Rx の X, Y, Z)"""
    sy = -m[2][0]
    y = math.asin(max(-1.0, min(1.0, sy)))
    if abs(math.cos(y)) > 1e-9:
        x = math.atan2(m[2][1], m[2][2])
        z = math.atan2(m[1][0], m[0][0])
    else:
        x = math.atan2(-m[1][2], m[1][1])
        z = 0.0
    return [round(math.degrees(a), 6) + 0.0 for a in (x, y, z)]


def r3(v):
    return [round(c, 3) + 0.0 for c in v]


HOME = k.ur5e_frames(k.UR5E_HOME)
TOOL0 = to_world(HOME["tool0"])
TCP = r3(k.apply(TOOL0, [0.0, 0.0, TCP_FROM_FLANGE * M]))
TOOL_AXIS = r3(k.apply_dir(TOOL0, [0.0, 0.0, 1.0]))

# ---------------------------------------------------------------------------
# アセンブリの木
# ---------------------------------------------------------------------------
UR_FILE = 0
GRIPPER_FILE = 1
ARM, CELL = 0, 1
ROBOT, GRIPPER = [ARM, 0], [ARM, 1]  # ロボットのアセンブリの中の UR5e とグリッパ


def node(name, kind, children=None, position=None, rotation=None, color=None):
    n = {
        "name": name,
        "position": position or [0.0, 0.0, 0.0],
        "rotation_deg": rotation or [0.0, 0.0, 0.0],
        "visible": True,
        "kind": kind,
    }
    if color:
        n["color"] = color
    if children:
        n["children"] = children
    return n


def link(file, name, label, colors):
    """読み込んだファイルのリンク (色ごとのメッシュを子に持つ)"""
    return node(
        label,
        {"type": "Assembly", "product": name},
        [node(f"{name}_{c}", {"type": "Imported", "step_file": file, "product": f"{name}_{c}"}) for c in colors],
    )


def box(name, size, center, color=None):
    return node(
        name,
        {"type": "Box", "size": size, "position": center, "rotation_deg": [0.0, 0.0, 0.0]},
        color=color,
    )


def standard(name, part, params, position, yaw=0.0):
    return node(name, {"type": "Standard", "part": part, "params": params}, position=position, rotation=[0.0, yaw, 0.0])


# GLB の中のリンクごとの色 (convert_meshes.py が付けた名前)
UR_COLORS = {
    "base": ["black", "gray"],
    "shoulder": ["black", "blue", "gray"],
    "upper_arm": ["black", "blue", "gray", "silver"],
    "forearm": ["black", "blue", "gray", "silver"],
    "wrist_1": ["black", "blue", "gray"],
    "wrist_2": ["black", "blue", "gray"],
    "wrist_3": ["silver"],
}
UR_LABELS = {
    "base": "ベース",
    "shoulder": "ショルダー",
    "upper_arm": "上腕",
    "forearm": "前腕",
    "wrist_1": "手首1",
    "wrist_2": "手首2",
    "wrist_3": "手首3 (フランジ)",
}
GRIPPER_COLORS = {
    "ur_to_robotiq_adapter": ["black", "gray"],
    "robotiq_base": ["black", "silver"],
    "left_knuckle": ["silver"],
    "right_knuckle": ["silver"],
    "left_finger": ["black", "silver"],
    "right_finger": ["black", "silver"],
    "left_inner_knuckle": ["black", "silver"],
    "right_inner_knuckle": ["black", "silver"],
    "left_finger_tip": ["black", "silver"],
    "right_finger_tip": ["black", "silver"],
}
GRIPPER_LABELS = {
    "ur_to_robotiq_adapter": "UR 用アダプタ",
    "robotiq_base": "本体",
    "left_knuckle": "左 外側ナックル",
    "right_knuckle": "右 外側ナックル",
    "left_finger": "左 フィンガー",
    "right_finger": "右 フィンガー",
    "left_inner_knuckle": "左 内側ナックル",
    "right_inner_knuckle": "右 内側ナックル",
    "left_finger_tip": "左 指先",
    "right_finger_tip": "右 指先",
}
GRIPPER_ORDER = [name for name, *_ in k.GRIPPER_LINKS]

robot = node(
    "UR5e (Universal Robots)",
    {"type": "Assembly", "product": "UR5e"},
    [link(UR_FILE, name, UR_LABELS[name], UR_COLORS[name]) for name in k.UR5E_LINKS],
    position=BASE,
)
gripper = node(
    "2F-85 (Robotiq)",
    {"type": "Assembly", "product": "Robotiq 2F-85"},
    [link(GRIPPER_FILE, name, GRIPPER_LABELS[name], GRIPPER_COLORS[name]) for name in GRIPPER_ORDER],
    position=r3(k.translation(TOOL0)),
    # グリッパのファイルは Y 上の座標で書いてあるので、tool0 の向き (ROS の座標) を Y 上に写して置く
    rotation=euler_deg(k.chain(TOOL0, k.YUP_TO_ZUP)),
)

IN_CENTER = IN_END - IN_LEN / 2
OUT_CENTER = OUT_START + OUT_LEN / 2
cell = node(
    "セル",
    {"type": "Assembly"},
    [
        box("ロボット架台", [320.0, PEDESTAL_H - 1.0, 320.0], [0.0, (PEDESTAL_H - 1.0) / 2, 0.0], DARK),  # 0
        standard(  # 1
            "供給コンベア",
            "belt_conveyor",
            {"length": IN_LEN, "width": BELT_W, "height": BELT_TOP, "speed": IN_SPEED},
            [LINE_X, 0.0, IN_CENTER],
            yaw=-90.0,
        ),
        standard(  # 2
            "排出コンベア",
            "belt_conveyor",
            {"length": OUT_LEN, "width": BELT_W, "height": BELT_TOP, "speed": 250.0},
            [LINE_X, 0.0, OUT_CENTER],
            yaw=-90.0,
        ),
        box("投入シュート", [360.0, 900.0, 200.0], [LINE_X, 450.0, IN_END - IN_LEN - 250.0], BLUE),  # 3
        box("回収箱", [400.0, 500.0, 400.0], [LINE_X, 250.0, OUT_START + OUT_LEN + 200.0], GREEN),  # 4
    ],
)

# 2 社の CAD をまとめるアセンブリ。メーカーの CAD はリンクの継ぎ目で形状が重なっているので、
# 中の部品どうしの干渉は調べない (コンベアやワークとの干渉は調べる)
arm = node("ロボット (UR5e + 2F-85)", {"type": "Assembly"}, [robot, gripper])
arm["skip_inner_interference"] = True

nodes = [arm, cell]

# ---------------------------------------------------------------------------
# 関節 (アクチュエータ)
# ---------------------------------------------------------------------------
J = [1, 2, 3, 4, 5, 6]
L_OUTER, L_INNER, L_TIP, R_OUTER, R_INNER, R_TIP, CHUCK = 11, 12, 13, 14, 15, 16, 20
JOINT_LABELS = ["J1 ベース", "J2 ショルダー", "J3 エルボー", "J4 手首1", "J5 手首2", "J6 手首3"]


def gripper_path(name):
    return GRIPPER + [GRIPPER_ORDER.index(name)]


def rotary(id_, name, frame_world, parts, parent=None, limits=None, flip=False, duration=1.0):
    """frame_world の Z 軸まわりに回る関節 (frame_world は設計位置でのワールドの変換)"""
    direction = k.apply_dir(frame_world, [0.0, 0.0, 1.0])
    if flip:
        direction = [-c for c in direction]
    axis, reversed_ = axis_letter(direction)
    a = {
        "id": id_,
        "name": name,
        "kind": "RotaryTable",
        "axis": axis,
        "stroke": 0.0,
        "pivot": r3(k.translation(frame_world)),
        "parts": parts,
        "easing": "Smooth",
        "default_duration": duration,
    }
    if reversed_:
        a["reversed"] = True
    if parent:
        a["mounted_on"] = parent
    if limits:
        a["limits"] = limits
    return a


actuators = []
# UR5e: 関節 i はリンク i+1 を動かす。手首3 にはグリッパの本体とアダプタも載る
for i, joint_name in enumerate(k.UR5E_JOINTS):
    link_name = k.UR5E_LINKS[i + 1]
    parts = [ROBOT + [i + 1]]
    if i == 5:
        parts += [gripper_path("ur_to_robotiq_adapter"), gripper_path("robotiq_base")]
    home_deg = math.degrees(k.UR5E_HOME[i])
    # UR5e の可動範囲は全関節 ±360°。値はホームからの角度なので、ホームの分をずらす
    limits = [round(-360.0 - home_deg, 3), round(360.0 - home_deg, 3)]
    actuators.append(
        rotary(J[i], JOINT_LABELS[i], to_world(HOME[link_name]), parts, J[i - 1] if i else None, limits)
    )

# グリッパの指: 関節の軸はグリッパの -Y (ROS)。開閉の角度に掛ける係数が負なら逆に回す
GRIPPER_FRAMES = k.gripper_frames(0.0)
GRIPPER_TO_WORLD = HOME["tool0"]
FINGER_IDS = {
    "left_knuckle": (L_OUTER, "左指 外側", J[5]),
    "left_inner_knuckle": (L_INNER, "左指 内側", J[5]),
    "left_finger_tip": (L_TIP, "左指 指先", L_OUTER),
    "right_knuckle": (R_OUTER, "右指 外側", J[5]),
    "right_inner_knuckle": (R_INNER, "右指 内側", J[5]),
    "right_finger_tip": (R_TIP, "右指 指先", R_OUTER),
}
for name, _parent, _xyz, factor in k.GRIPPER_LINKS:
    if name not in FINGER_IDS:
        continue
    id_, label, parent = FINGER_IDS[name]
    # 関節の座標: Z をグリッパの関節の軸 (-Y) に向けた座標
    joint_frame = k.chain(GRIPPER_TO_WORLD, GRIPPER_FRAMES[name], k.origin(r=(math.pi / 2, 0.0, 0.0)))
    parts = [gripper_path(name)]
    if name.endswith("_knuckle") and not name.endswith("inner_knuckle"):
        parts.append(gripper_path(name.replace("knuckle", "finger")))
    max_deg = round(math.degrees(k.GRIPPER_MAX_ANGLE), 3)
    actuators.append(
        rotary(
            id_,
            label,
            to_world(joint_frame),
            parts,
            parent,
            [0.0, max_deg],
            flip=factor < 0,
            duration=0.4,
        )
    )

# 掴む・離すの合図にするシリンダ (動かす部品はない)。出になり切ったら掴み、戻り始めたら離す
actuators.append(
    {
        "id": CHUCK,
        "name": "把持 (ワークを掴む合図)",
        "kind": "Cylinder",
        "axis": "Y",
        "stroke": 1.0,
        "pivot": [0.0, 0.0, 0.0],
        "mounted_on": J[5],
        "parts": [],
        "easing": "Smooth",
        "default_duration": 0.4,
    }
)


# ---------------------------------------------------------------------------
# 逆運動学 (大きな移動の関節角を求める。上下の出し入れはアプリの直線補間に任せる)
# ---------------------------------------------------------------------------


def tcp_and_axis(offsets):
    q = [h + o for h, o in zip(k.UR5E_HOME, offsets)]
    tool0 = to_world(k.ur5e_frames(q)["tool0"])
    return k.apply(tool0, [0.0, 0.0, TCP_FROM_FLANGE * M]), k.apply_dir(tool0, [0.0, 0.0, 1.0])


def solve(target, start, approach=(0.0, -1.0, 0.0)):
    """TCP を target に置き、工具を approach に向ける関節角 (ホームからの rad)。減衰最小二乗法"""
    q = list(start)
    for _ in range(200):
        p, a = tcp_and_axis(q)
        err = [t - c for t, c in zip(target, p)]
        cross = [a[1] * approach[2] - a[2] * approach[1], a[2] * approach[0] - a[0] * approach[2], a[0] * approach[1] - a[1] * approach[0]]
        e = err + [-c * 300.0 for c in cross]  # 向きの誤差 (0 にしたい) を mm にそろえる
        if max(abs(c) for c in err) < 1e-2 and max(abs(c) for c in cross) < 1e-5:
            return q
        # 数値微分のヤコビアン (6 x 6)
        cols = []
        for j in range(6):
            dq = list(q)
            dq[j] += 1e-6
            p2, a2 = tcp_and_axis(dq)
            c2 = [a2[1] * approach[2] - a2[2] * approach[1], a2[2] * approach[0] - a2[0] * approach[2], a2[0] * approach[1] - a2[1] * approach[0]]
            row = [(x - y) / 1e-6 for x, y in zip(p2, p)] + [(x - y) * 300.0 / 1e-6 for x, y in zip(c2, cross)]
            cols.append(row)
        jac = [[cols[j][i] for j in range(6)] for i in range(6)]
        # Δq = Jᵀ (J Jᵀ + λ² I)⁻¹ e
        lam2 = 1.0
        a_mat = [[sum(jac[i][m] * jac[r][m] for m in range(6)) + (lam2 if i == r else 0.0) for r in range(6)] for i in range(6)]
        y = gauss(a_mat, e)
        dq = [sum(jac[i][j] * y[i] for i in range(6)) for j in range(6)]
        step = max(abs(d) for d in dq)
        if step > 0.2:
            dq = [d * 0.2 / step for d in dq]
        q = [a + b for a, b in zip(q, dq)]
    raise RuntimeError(f"逆運動学が収束しませんでした: {target}")


def gauss(a, b):
    n = len(b)
    m = [row[:] + [b[i]] for i, row in enumerate(a)]
    for c in range(n):
        p = max(range(c, n), key=lambda r: abs(m[r][c]))
        m[c], m[p] = m[p], m[c]
        for r in range(n):
            if r != c:
                f = m[r][c] / m[c][c]
                m[r] = [x - f * y for x, y in zip(m[r], m[c])]
    return [m[i][n] / m[i][i] for i in range(n)]


ZERO = [0.0] * 6
PICK_ABOVE = [PICK[0], PICK[1] + LIFT, PICK[2]]
PLACE_ABOVE = [PLACE[0], PLACE[1] + LIFT, PLACE[2]]
Q_PICK_ABOVE = solve(PICK_ABOVE, ZERO)
Q_PLACE_ABOVE = solve(PLACE_ABOVE, Q_PICK_ABOVE)

# ---------------------------------------------------------------------------
# ロボット・教示点・ステップ
# ---------------------------------------------------------------------------
DOWN = [0.0, -1.0, 0.0]
points = [
    {"id": 1, "name": "ピック上", "position": r3(PICK_ABOVE), "approach": DOWN},
    {"id": 2, "name": "ピック", "position": r3(PICK), "approach": DOWN},
    {"id": 3, "name": "プレース上", "position": r3(PLACE_ABOVE), "approach": DOWN},
    {"id": 4, "name": "プレース", "position": r3(PLACE), "approach": DOWN},
]
robots = [
    {
        "id": 1,
        "name": "UR5e",
        "tip": J[5],
        "tcp": TCP,
        "tool_axis": TOOL_AXIS,
        "points": points,
        "joint_speed": 180.0,
        "accel_time": 0.2,
    }
]


def act(actuator, target, duration):
    return {"actuator": actuator, "target": round(target, 3) + 0.0, "duration": duration}


def go(q, duration):
    return [act(j, math.degrees(v), duration) for j, v in zip(J, q)]


def mv(point, duration):
    return {"robot": 1, "point": point, "duration": duration, "kind": "Linear"}


def fingers(angle, duration):
    deg = math.degrees(angle)
    return [act(a, deg, duration) for a in (L_OUTER, L_INNER, L_TIP, R_OUTER, R_INNER, R_TIP)]


steps = [
    # ワークがストッパに着くまで待つ (ワーク搬送のワークはセンサーで検知できないので、時間で合わせる)
    {"comment": "ピック上へ (ワーク待ち)", "actions": go(Q_PICK_ABOVE, 1.0), "dwell": FIRST_DWELL},
    {"comment": "下降", "moves": [mv(2, 0.6)]},
    {"comment": "把持", "actions": fingers(GRIP_ANGLE, 0.4) + [act(CHUCK, 1.0, 0.4)], "dwell": 0.1},
    {"comment": "上昇", "moves": [mv(1, 0.6)]},
    {"comment": "プレース上へ", "actions": go(Q_PLACE_ABOVE, 1.2)},
    {"comment": "下降", "moves": [mv(4, 0.6)]},
    {"comment": "解放", "actions": fingers(0.0, 0.4) + [act(CHUCK, 0.0, 0.4)], "dwell": 0.1},
    {"comment": "上昇", "moves": [mv(3, 0.6)]},
    {"comment": "ホームへ", "actions": go(ZERO, 1.0)},
]

chart = {"actuators": actuators, "steps": steps, "robots": robots}


def step_time(step):
    durations = [a["duration"] for a in step.get("actions", [])] + [m["duration"] for m in step.get("moves", [])]
    return max(durations) + step.get("dwell", 0.0)


# 1 サイクルに 1 個ずつ流れるよう、投入の間隔をサイクルタイムにそろえる
CYCLE = round(sum(step_time(s) for s in steps), 3)

conveyors = {
    "kinds": [{"id": 1, "name": "ワーク", "size": WORK, "color": [0.95, 0.55, 0.15]}],
    "sources": [
        {
            "id": 1,
            "name": "投入シュート",
            "part": [CELL, 3],
            "output": {"type": "Conveyor", "conveyor": 1},
            "interval": CYCLE,
            "kinds": [1],
        }
    ],
    "sinks": [{"id": 1, "name": "回収箱", "part": [CELL, 4]}],
    "conveyors": [
        {"id": 1, "name": "供給コンベア", "part": [CELL, 1], "axis": "Z", "speed": IN_SPEED, "end": "Stopper"},
        {"id": 2, "name": "排出コンベア", "part": [CELL, 2], "axis": "Z", "speed": 250.0, "end": "Discharge"},
    ],
    "grippers": [{"id": 1, "name": "2F-85", "chuck": CHUCK, "tcp": TCP, "reach": 40.0}],
}

project = {
    "format_version": 1,
    "unit": "mm",
    "step_files": [
        {"type": "File", "path": "ur5e.glb"},
        {"type": "File", "path": "robotiq_2f85.glb"},
    ],
    "nodes": nodes,
    "chart": chart,
    "conveyors": conveyors,
}


def main():
    root = Path(__file__).resolve().parents[2]
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else root / "samples" / "real_robot" / "ur5e_pick_place.plantwright"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(project, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"{out} に書きました")
    print("ピック上:", [round(math.degrees(v), 1) for v in Q_PICK_ABOVE])
    print("プレース上:", [round(math.degrees(v), 1) for v in Q_PLACE_ABOVE])
    print(f"掴む角度: {math.degrees(GRIP_ANGLE):.1f}°  TCP: {TCP}  工具の向き: {TOOL_AXIS}")


if __name__ == "__main__":
    main()
