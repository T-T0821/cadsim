#!/usr/bin/env python3
"""デモ用のライン (samples/demo_line.cadsim) を作る。

供給コンベア → 4軸ロボットの移載 → 排出コンベア → AGV で検査場へ → 出荷、という小さな組立ラインです。
標準部品・タイミングチャート (ロボットの教示点とセンサー)・ワーク搬送・安全機器・作業者・AGV・
工程モデル (人員配置付き)・寸法と注記・ケーブルを1つのプロジェクトにまとめます。

    python3 tools/make_demo_line.py            # samples/demo_line.cadsim に書く
    python3 tools/make_demo_line.py out.cadsim

座標は mm、Y が上。床の上の (x, z) で配置を考えています。
"""

import json
import math
import sys
from pathlib import Path

# ---------------------------------------------------------------------------
# 寸法
# ---------------------------------------------------------------------------
BELT_TOP = 800.0  # コンベアの搬送面の高さ
BELT_W = 400.0
LINE_Z = 700.0  # コンベアの中心線 (ロボットの手前)
WORK = [180.0, 120.0, 180.0]
GRIP_Y = BELT_TOP + WORK[1] / 2  # ワークを掴む高さ (ワークの中心)

# ロボットの関節位置 (設計位置では腕がまっすぐ上を向く)
J2_Y = 950.0
J3_Y = 1700.0
J4_Y = 2350.0
TCP_Y = 2740.0

ORANGE = [0.98, 0.52, 0.12]
DARK = [0.22, 0.24, 0.28]
GREY = [0.62, 0.65, 0.70]
STEEL = [0.78, 0.80, 0.84]
YELLOW = [0.96, 0.80, 0.16]
GREEN = [0.30, 0.70, 0.45]
BLUE = [0.25, 0.50, 0.85]
WOOD = [0.72, 0.56, 0.38]


def box(name, size, center, color=None, rot=None):
    node = {
        "name": name,
        "position": [0.0, 0.0, 0.0],
        "rotation_deg": [0.0, 0.0, 0.0],
        "visible": True,
        "kind": {
            "type": "Box",
            "size": size,
            "position": center,
            "rotation_deg": rot or [0.0, 0.0, 0.0],
        },
    }
    if color:
        node["color"] = color
    return node


def standard(name, part, params, position, yaw=0.0, color=None):
    node = {
        "name": name,
        "position": position,
        "rotation_deg": [0.0, yaw, 0.0],
        "visible": True,
        "kind": {"type": "Standard", "part": part, "params": params},
    }
    if color:
        node["color"] = color
    return node


def assembly(name, children, color=None):
    node = {
        "name": name,
        "position": [0.0, 0.0, 0.0],
        "rotation_deg": [0.0, 0.0, 0.0],
        "visible": True,
        "kind": {"type": "Assembly"},
        "children": children,
    }
    if color:
        node["color"] = color
    return node


# ---------------------------------------------------------------------------
# アセンブリの木 (順番が子の番号になるので、並べ替えるときは下の PATH も直す)
# ---------------------------------------------------------------------------
robot_cell = assembly(
    "ロボットセル",
    [
        box("架台", [560.0, 600.0, 560.0], [0.0, 300.0, 0.0], DARK),  # 0
        box("旋回ベース", [460.0, 200.0, 460.0], [0.0, 700.0, 0.0], ORANGE),  # 1
        # 隣り合うリンクは Z にずらして、関節が回っても重ならないようにする
        box("肩", [300.0, 300.0, 200.0], [0.0, J2_Y, 0.0], ORANGE),  # 2
        box("上腕", [170.0, J3_Y - J2_Y + 180.0, 170.0], [0.0, (J2_Y + J3_Y) / 2, 185.0], ORANGE),  # 3
        box("前腕", [140.0, 460.0, 140.0], [0.0, 2010.0, 0.0], ORANGE),  # 4
        box("手首", [180.0, 200.0, 180.0], [0.0, J4_Y + 100.0, 0.0], DARK),  # 5
        box("ハンド", [320.0, 60.0, 160.0], [0.0, 2580.0, 0.0], STEEL),  # 6
        box("固定爪", [30.0, 150.0, 120.0], [-105.0, 2685.0, 0.0], STEEL),  # 7
        box("可動爪", [30.0, 150.0, 120.0], [155.0, 2685.0, 0.0], STEEL),  # 8
        box("移載台", [600.0, 760.0, 500.0], [0.0, 380.0, LINE_Z], GREY),  # 9
    ],
)

conveying = assembly(
    "搬送",
    [
        standard(  # 0
            "供給コンベア",
            "belt_conveyor",
            {"length": 2400.0, "width": BELT_W, "height": BELT_TOP, "speed": 300.0},
            [-1700.0, 0.0, LINE_Z],
        ),
        standard(  # 1
            "排出コンベア",
            "belt_conveyor",
            {"length": 2400.0, "width": BELT_W, "height": BELT_TOP, "speed": 250.0},
            [1700.0, 0.0, LINE_Z],
        ),
        box("投入シュート", [300.0, 1100.0, 500.0], [-3080.0, 550.0, LINE_Z], BLUE),  # 2
        box("排出箱", [400.0, 600.0, 600.0], [3150.0, 300.0, LINE_Z], GREEN),  # 3
        standard(  # 4 供給コンベアの終端 (手前側からワークを見る)
            "ワーク到着センサー",
            "photo_sensor",
            {"range": 500.0},
            [-700.0, BELT_TOP + 20.0, LINE_Z + 330.0],
            yaw=90.0,
        ),
        standard(  # 5
            "排出確認センサー",
            "photo_sensor",
            {"range": 500.0},
            [2600.0, BELT_TOP + 20.0, LINE_Z + 330.0],
            yaw=90.0,
        ),
        standard(  # 6 排出コンベアの横のプッシャ (向きそろえ)
            "整列シリンダ",
            "air_cylinder",
            {"bore": 40.0, "stroke": 150.0},
            [1300.0, BELT_TOP + 40.0, LINE_Z - 420.0],
            yaw=-90.0,
        ),
    ],
)

control = assembly(
    "制御盤まわり",
    [
        standard(  # 0
            "制御盤",
            "control_panel",
            {"width": 800.0, "height": 1800.0, "depth": 400.0},
            [-3000.0, 0.0, -1600.0],
        ),
        standard("シーケンサー", "plc", {"inputs": 32.0, "outputs": 32.0}, [-3150.0, 1200.0, -1640.0]),  # 1
        standard("電源 24V", "power_supply", {"watts": 240.0}, [-2850.0, 1200.0, -1640.0]),  # 2
        standard("リモート I/O", "remote_io", {"inputs": 16.0, "outputs": 16.0}, [-3150.0, 800.0, -1640.0]),  # 3
        standard("リレー", "relay", {}, [-2850.0, 800.0, -1640.0]),  # 4
        box("操作台", [400.0, 1000.0, 300.0], [-3000.0, 500.0, 2400.0], DARK),  # 5
        standard("起動ボタン", "push_button", {}, [-3050.0, 1000.0, 2400.0]),  # 6
        standard("停止ボタン", "push_button", {}, [-2950.0, 1000.0, 2400.0]),  # 7
        box("表示灯ポール", [60.0, 1900.0, 60.0], [2450.0, 950.0, 2250.0], STEEL),  # 8
        standard("シグナルタワー", "signal_tower", {"tiers": 3.0}, [2450.0, 1900.0, 2250.0]),  # 9
    ],
)

area = assembly(
    "周辺設備",
    [
        box("部品棚", [1120.0, 30.0, 450.0], [-4300.0, 1785.0, 2600.0], WOOD),  # 0 天板
        box("部品棚 (中段)", [1120.0, 30.0, 450.0], [-4300.0, 900.0, 2600.0], WOOD),  # 1
        box("検査台", [1200.0, 850.0, 700.0], [-1800.0, 425.0, 4400.0], STEEL),  # 2
        box("検査カメラ", [160.0, 160.0, 160.0], [-1800.0, 1500.0, 4400.0], DARK),  # 3
        box("カメラ支柱", [60.0, 650.0, 60.0], [-1800.0, 1175.0, 4720.0], STEEL),  # 4
        box("出荷パレット", [1100.0, 150.0, 1100.0], [-3800.0, 75.0, 4600.0], WOOD),  # 5
        box("充電ステーション", [500.0, 400.0, 300.0], [4600.0, 200.0, 5400.0], YELLOW),  # 6
        box("部品棚 (下段)", [1120.0, 30.0, 450.0], [-4300.0, 150.0, 2600.0], WOOD),  # 7
    ]
    + [
        box(f"部品棚 柱{i + 1}", [40.0, 1770.0, 40.0], [-4300.0 + dx, 885.0, 2600.0 + dz], DARK)
        for i, (dx, dz) in enumerate([(-580, -205), (580, -205), (-580, 205), (580, 205)])
    ],
)

nodes = [robot_cell, conveying, control, area]

ROBOT = 0
CONV = 1
CTRL = 2
AREA = 3


def path(asm, child):
    return [asm, child]


# ---------------------------------------------------------------------------
# タイミングチャート: 4軸ロボットでワークを供給コンベアから排出コンベアへ移す
# ---------------------------------------------------------------------------
J1, J2, J3, J4, GRIP, PUSH = 1, 2, 3, 4, 5, 6


def joint(id_, name, axis, pivot, parent, parts, limit):
    return {
        "id": id_,
        "name": name,
        "kind": "RotaryTable",
        "axis": axis,
        "stroke": 0.0,
        "pivot": pivot,
        **({"mounted_on": parent} if parent else {}),
        "parts": [path(ROBOT, p) for p in parts],
        "easing": "Smooth",
        "default_duration": 1.0,
        "limits": [-limit, limit],
    }


actuators = [
    joint(J1, "J1 旋回", "Y", [0.0, 0.0, 0.0], None, [1, 2], 170.0),
    joint(J2, "J2 肩", "Z", [0.0, J2_Y, 0.0], J1, [3], 120.0),
    joint(J3, "J3 肘", "Z", [0.0, J3_Y, 0.0], J2, [4], 150.0),
    joint(J4, "J4 手首", "Z", [0.0, J4_Y, 0.0], J3, [5, 6, 7], 135.0),
    {
        "id": GRIP,
        "name": "グリッパ",
        "kind": "Cylinder",
        "axis": "X",
        "reversed": True,
        "stroke": 50.0,
        "pivot": [0.0, 0.0, 0.0],
        "mounted_on": J4,
        "parts": [path(ROBOT, 8)],
        "easing": "Smooth",
        "default_duration": 0.3,
    },
    {
        "id": PUSH,
        "name": "整列シリンダ",
        "kind": "Cylinder",
        "axis": "Z",
        "stroke": 120.0,
        "pivot": [0.0, 0.0, 0.0],
        "parts": [path(CONV, 6)],
        "easing": {"Trapezoid": {"accel": 0.25}},
        "default_duration": 0.4,
    },
]

GRIP_CLOSED = 1  # センサーの id
sensors = [
    {
        "id": GRIP_CLOSED,
        "name": "グリッパ閉確認",
        "kind": "Proximity",
        # 閉じた可動爪 (X- に 50 動いて 90..120) の中。開くと爪が検出距離の外へ出る。手首と一緒に回る
        "position": [100.0, 2685.0, 0.0],
        "axis": "X",
        "range": 30.0,
        "mounted_on": J4,
    }
]

DOWN = [0.0, -1.0, 0.0]
PICK = [-600.0, GRIP_Y, LINE_Z]
PLACE = [650.0, GRIP_Y, LINE_Z]
LIFT = 280.0
points = [
    {"id": 1, "name": "ピック上", "position": [PICK[0], PICK[1] + LIFT, PICK[2]], "approach": DOWN},
    {"id": 2, "name": "ピック", "position": PICK, "approach": DOWN},
    {"id": 3, "name": "プレース上", "position": [PLACE[0], PLACE[1] + LIFT, PLACE[2]], "approach": DOWN},
    {"id": 4, "name": "プレース", "position": PLACE, "approach": DOWN},
]
robots = [
    {
        "id": 1,
        "name": "移載ロボット",
        "tip": J4,
        "tcp": [0.0, TCP_Y, 0.0],
        "tool_axis": [0.0, 1.0, 0.0],
        "points": points,
        "joint_speed": 180.0,
        "accel_time": 0.2,
    }
]


def mv(point, duration, linear=False):
    m = {"robot": 1, "point": point, "duration": duration}
    if linear:
        m["kind"] = "Linear"
    return m


def act(actuator, target, duration):
    return {"actuator": actuator, "target": target, "duration": duration}


def arm_pose(target):
    """工具を下に向けて TCP を target に置く関節角 (度)。肘を上にした解。"""
    x, y, z = target
    l1, l2, l3 = J3_Y - J2_Y, J4_Y - J3_Y, TCP_Y - J4_Y
    r = math.hypot(x, z)
    dx, dy = r, y + l3 - J2_Y
    d = math.hypot(dx, dy)
    phi = math.degrees(math.atan2(dx, dy))
    beta = math.degrees(math.acos((l1 * l1 + d * d - l2 * l2) / (2 * l1 * d)))
    a3 = math.degrees(math.acos((d * d - l1 * l1 - l2 * l2) / (2 * l1 * l2)))
    a2 = phi - beta
    a4 = 180.0 - a2 - a3
    # J2 を + に回すと腕はローカルの -X 側へ倒れる。その向きを target に向ける
    a1 = math.degrees(math.atan2(z / r, -x / r))
    return [a1, a2, a3, a4]


def go(target, duration):
    return [act(j, round(v, 3), duration) for j, v in zip((J1, J2, J3, J4), arm_pose(target))]


PICK_ABOVE = [PICK[0], PICK[1] + LIFT, PICK[2]]
PLACE_ABOVE = [PLACE[0], PLACE[1] + LIFT, PLACE[2]]

# 大きく動くところは関節の動作で、上下の出し入れは教示点への直線補間で動かす
steps = [
    {"comment": "ピック上へ", "actions": go(PICK_ABOVE, 1.0)},
    {"comment": "下降", "moves": [mv(2, 0.5, True)]},
    {"comment": "把持", "actions": [act(GRIP, 1.0, 0.3)], "dwell": 0.1},
    {"comment": "上昇", "moves": [mv(1, 0.5, True)], "interlocks": [{"sensor": GRIP_CLOSED, "on": True}]},
    {"comment": "プレース上へ", "actions": go(PLACE_ABOVE, 1.2)},
    {"comment": "下降", "moves": [mv(4, 0.5, True)]},
    {"comment": "解放", "actions": [act(GRIP, 0.0, 0.3)], "dwell": 0.1},
    {
        "comment": "上昇・整列",
        "moves": [mv(3, 0.5, True)],
        "actions": [act(PUSH, 1.0, 0.4)],
        "interlocks": [{"sensor": GRIP_CLOSED, "on": False}],
    },
    {"comment": "待機位置へ", "actions": [act(PUSH, 0.0, 0.4)] + [act(j, 0.0, 1.0) for j in (J1, J2, J3, J4)]},
]

chart = {
    "actuators": actuators,
    "sensors": sensors,
    "steps": steps,
    "robots": robots,
}

# ---------------------------------------------------------------------------
# ワーク搬送
# ---------------------------------------------------------------------------
conveyors = {
    "kinds": [
        {"id": 1, "name": "ハウジング", "size": WORK, "color": [0.20, 0.55, 0.95]},
        {"id": 2, "name": "カバー", "size": [WORK[0], WORK[1] * 0.6, WORK[2]], "color": [0.95, 0.35, 0.40]},
    ],
    "sources": [
        {
            "id": 1,
            "name": "投入シュート",
            "part": path(CONV, 2),
            "output": {"type": "Conveyor", "conveyor": 1},
            "interval": 6.6,
            "kinds": [1, 2],
        }
    ],
    "sinks": [{"id": 1, "name": "排出箱", "part": path(CONV, 3)}],
    "conveyors": [
        {"id": 1, "name": "供給コンベア", "part": path(CONV, 0), "axis": "X", "speed": 300.0, "end": "Stopper"},
        {"id": 2, "name": "排出コンベア", "part": path(CONV, 1), "axis": "X", "speed": 250.0, "end": "Discharge"},
    ],
    "grippers": [
        {"id": 1, "name": "ロボットハンド", "chuck": GRIP, "tcp": [0.0, TCP_Y - 60.0, 0.0], "reach": 120.0}
    ],
}

# ---------------------------------------------------------------------------
# 床: 安全機器・作業者・AGV
# ---------------------------------------------------------------------------
CELL_X = 2400.0
CELL_BACK = -1900.0
CELL_FRONT = 2200.0


def fence(id_, name, x0, z0, x1, z1):
    length = ((x1 - x0) ** 2 + (z1 - z0) ** 2) ** 0.5
    yaw = 0.0 if abs(z1 - z0) < 1e-6 else 90.0
    return {
        "id": id_,
        "name": name,
        "kind": "Fence",
        "position": [(x0 + x1) / 2, 0.0, (z0 + z1) / 2],
        "yaw_deg": yaw,
        "size": [length, 1800.0, 50.0],
        "stop_time": 0.2,
        "resolution": 30.0,
        "required_clearance": 850.0,
    }


safety = [
    fence(1, "安全柵 (奥)", -CELL_X, CELL_BACK, CELL_X, CELL_BACK),
    fence(2, "安全柵 (左)", -CELL_X, CELL_BACK, -CELL_X, LINE_Z - 300.0),
    fence(3, "安全柵 (右)", CELL_X, CELL_BACK, CELL_X, LINE_Z - 300.0),
    fence(4, "安全柵 (手前左)", -CELL_X, CELL_FRONT, -800.0, CELL_FRONT),
    fence(5, "安全柵 (手前右)", 800.0, CELL_FRONT, CELL_X, CELL_FRONT),
    fence(9, "安全柵 (左手前)", -CELL_X, LINE_Z + 300.0, -CELL_X, CELL_FRONT),
    fence(10, "安全柵 (右手前)", CELL_X, LINE_Z + 300.0, CELL_X, CELL_FRONT),
    {
        "id": 6,
        "name": "ライトカーテン (扉)",
        "kind": "LightCurtain",
        "position": [0.0, 300.0, CELL_FRONT],
        "yaw_deg": 0.0,
        "size": [1600.0, 1500.0, 30.0],
        "stop_time": 0.25,
        "resolution": 30.0,
        "required_clearance": 850.0,
    },
    {
        "id": 7,
        "name": "非常停止 (操作台)",
        "kind": "EmergencyStop",
        "position": [-2650.0, 0.0, 2400.0],
        "yaw_deg": 0.0,
        "size": [80.0, 1100.0, 80.0],
        "stop_time": 0.2,
        "resolution": 30.0,
        "required_clearance": 850.0,
    },
    {
        "id": 8,
        "name": "エリアセンサ (保守口)",
        "kind": "AreaSensor",
        "position": [0.0, 0.0, CELL_BACK - 900.0],
        "yaw_deg": 0.0,
        "size": [2000.0, 300.0, 1400.0],
        "stop_time": 0.2,
        "resolution": 30.0,
        "required_clearance": 850.0,
    },
]

operators = [
    {
        "id": 1,
        "name": "投入担当",
        "start": [-3900.0, 2000.0],
        "walk_speed": 1100.0,
        "tasks": [
            {"type": "Walk", "to": [-4300.0, 2150.0]},
            {"type": "Pick", "label": "部品棚から取る", "duration": 3.0},
            {"type": "Walk", "to": [-3080.0, 1250.0]},
            {"type": "Pick", "label": "シュートに投入", "duration": 2.5},
            {"type": "Walk", "to": [-3000.0, 2700.0]},
            {"type": "Button", "label": "起動", "duration": 1.0},
            {"type": "Walk", "to": [-3900.0, 2000.0]},
        ],
    },
    {
        "id": 2,
        "name": "検査担当",
        "start": [-1800.0, 3700.0],
        "walk_speed": 1000.0,
        "tasks": [
            {"type": "Walk", "to": [-800.0, 3700.0]},
            {"type": "Pick", "label": "AGV から受け取る", "duration": 3.0},
            {"type": "Walk", "to": [-1800.0, 3800.0]},
            {"type": "Button", "label": "検査", "duration": 6.0},
            {"type": "Walk", "to": [-3800.0, 3900.0]},
            {"type": "Pick", "label": "パレットに積む", "duration": 3.0},
            {"type": "Walk", "to": [-1800.0, 3700.0]},
        ],
    },
]

agv_nodes = [
    {"id": 1, "name": "セル出口", "pos": [3700.0, 1600.0], "station": True},
    {"id": 2, "name": "交差点 東", "pos": [3700.0, 3300.0], "station": False},
    {"id": 3, "name": "検査場", "pos": [-600.0, 3300.0], "station": True},
    {"id": 4, "name": "交差点 西", "pos": [-5200.0, 3300.0], "station": False},
    {"id": 5, "name": "部品供給", "pos": [-5200.0, 1300.0], "station": True},
    {"id": 6, "name": "充電", "pos": [3700.0, 5400.0], "station": True},
]
agv = {
    "nodes": agv_nodes,
    "edges": [[1, 2], [2, 3], [3, 4], [4, 5], [2, 6]],
    "flows": [
        {"from": 1, "to": 3, "interval": 40.0},
        {"from": 5, "to": 1, "interval": 90.0},
    ],
    "count": 2,
    "speed": 800.0,
    "load_time": 4.0,
    "unload_time": 4.0,
    "length": 900.0,
    "width": 600.0,
    "gap": 300.0,
}

floor = {"safety": safety, "operators": operators, "agv": agv}

# ---------------------------------------------------------------------------
# 工程モデル (工程シミュレーション・ライン連携・人員配置)
# ---------------------------------------------------------------------------
HOUR = 3600.0
flow = {
    "duration": 8 * HOUR,
    "warmup": 600.0,
    "seed": 42,
    "elements": [
        {
            "id": 1,
            "name": "部品投入",
            "type": "Source",
            "interval": 7.0,
            "spread": 0.1,
            "next": [2],
            "part": path(CONV, 2),
            "labor": {"type": "Load", "seconds": 3.0},
        },
        {
            "id": 2,
            "name": "ロボット移載",
            "type": "Process",
            "cycle_time": 6.6,
            "spread": 0.05,
            "mtbf": 3600.0,
            "mttr": 120.0,
            "next": [3],
            "part": [ROBOT],
            "rated_kw": 2.2,
            "standby_kw": 0.3,
            "air_l_per_min": 40.0,
            "starts_chart": True,
        },
        {
            "id": 3,
            "name": "排出コンベア",
            "type": "Buffer",
            "capacity": 8,
            "next": [4],
            "part": path(CONV, 1),
            "agv": {"from": 1, "to": 3},
        },
        {
            "id": 4,
            "name": "外観検査",
            "type": "Process",
            "cycle_time": 7.5,
            "spread": 0.2,
            "next": [5],
            "part": path(AREA, 2),
            "rated_kw": 0.4,
            "standby_kw": 0.1,
            "labor": {"type": "Full"},
        },
        {
            "id": 5,
            "name": "出荷",
            "type": "Sink",
            "part": path(AREA, 5),
        },
    ],
    "staffing": {
        "start_clock": 8 * HOUR,
        "walk_seconds": 5.0,
        "shifts": [
            {"id": 1, "name": "日勤", "start": 8 * HOUR, "end": 17 * HOUR, "breaks": [[12 * HOUR, 13 * HOUR]]},
            {"id": 2, "name": "早番", "start": 8 * HOUR, "end": 12 * HOUR},
        ],
        "workers": [
            {"id": 1, "name": "佐藤", "shift": 1, "skills": [[1, 1.0], [4, 1.1]], "primary": 1},
            {"id": 2, "name": "鈴木", "shift": 1, "skills": [[4, 1.0]], "primary": 4},
            {"id": 3, "name": "高橋", "shift": 2, "skills": [[1, 0.9], [4, 0.8]]},
        ],
    },
}

# ---------------------------------------------------------------------------
# 寸法・注記とケーブル
# ---------------------------------------------------------------------------
annotations = [
    {
        "type": "PointDistance",
        # 供給コンベアの終端から排出コンベアの始端まで (移載台の上面の両端)
        "a": {"node": path(ROBOT, 9), "local": [-1.0, 1.0, 1.0]},
        "b": {"node": path(ROBOT, 9), "local": [1.0, 1.0, 1.0]},
    },
    {
        "type": "Note",
        "at": {"node": path(ROBOT, 1), "local": [1.0, 1.0, 1.0]},
        "text": "4軸ロボット\n可搬 5 kg",
    },
    {
        "type": "Note",
        "at": {"node": path(AREA, 2), "local": [1.0, 1.0, -1.0]},
        "text": "外観検査\n(作業者 1 名)",
    },
]

cables = [
    {
        "name": "ロボットケーブル",
        "hose": False,
        "start": {"local": [-2700.0, 100.0, -1450.0]},
        "end": {"local": [-280.0, 150.0, -150.0]},
        "length": 3200.0,
        "diameter": 25.0,
        "min_bend_radius": 150.0,
    },
    {
        "name": "エアホース (整列シリンダ)",
        "hose": True,
        "start": {"local": [-2700.0, 300.0, -1450.0]},
        "end": {"local": [1150.0, BELT_TOP + 60.0, LINE_Z - 420.0]},
        "length": 5200.0,
        "diameter": 8.0,
        "min_bend_radius": 40.0,
    },
]

project = {
    "format_version": 1,
    "unit": "mm",
    "step_files": [],
    "nodes": nodes,
    "annotations": annotations,
    "flow": flow,
    "floor": floor,
    "cables": cables,
    "chart": chart,
    "conveyors": conveyors,
}


def main():
    root = Path(__file__).resolve().parent.parent
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else root / "samples" / "demo_line.cadsim"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(project, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"{out} に書きました")


if __name__ == "__main__":
    main()
