#!/usr/bin/env python3
"""本物の CAD のロボットセルを中心にしたデモライン (samples/real_robot/real_line.plantwright) を作る。

UR5e + Robotiq 2F-85 のピック & プレースのセル (ur5e_pick_place.plantwright) はそのまま使い、
まわりに安全柵・ライトカーテン・非常停止・エリアセンサ、制御盤と操作台、部品棚・検査台・出荷パレット、
作業者 2 人と AGV、工程モデル (工程シミュレーション・ライン連携・人員配置) を置く。
制御盤まわりと周辺設備の部品は samples/demo_line.plantwright から、置き場所だけ変えて持ってくる。

    python3 tools/real_robot/make_demo.py      # 先にセルを作る (なければ)
    python3 tools/real_robot/make_real_line.py

座標は mm、Y が上。ロボットの台座の中心が原点で、ワークは供給コンベア (-Z 側) から排出コンベア (+Z 側) へ流れる。
ロボットは -X 側、作業者が歩く通路は +X 側。
"""

import copy
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CELL = ROOT / "samples" / "real_robot" / "ur5e_pick_place.plantwright"
DEMO = ROOT / "samples" / "demo_line.plantwright"
OUT = ROOT / "samples" / "real_robot" / "real_line.plantwright"

# ---------------------------------------------------------------------------
# デモラインから持ってくる部品と、置き直す場所 (元の位置からのずらし量 X, Z)
# ---------------------------------------------------------------------------

PANEL_SHIFT = {  # 制御盤まわり
    "制御盤": (1500.0, -1000.0),
    "シーケンサー": (1500.0, -1000.0),
    "電源 24V": (1500.0, -1000.0),
    "リモート I/O": (1500.0, -1000.0),
    "リレー": (1500.0, -1000.0),
    "操作台": (4650.0, -4900.0),
    "起動ボタン": (4650.0, -4900.0),
    "停止ボタン": (4650.0, -4900.0),
    "表示灯ポール": (-950.0, -650.0),
    "シグナルタワー": (-950.0, -650.0),
}

SITE_SHIFT = {  # 周辺設備
    "部品棚": (5800.0, -5700.0),
    "部品棚 (中段)": (5800.0, -5700.0),
    "部品棚 (下段)": (5800.0, -5700.0),
    "部品棚 柱1": (5800.0, -5700.0),
    "部品棚 柱2": (5800.0, -5700.0),
    "部品棚 柱3": (5800.0, -5700.0),
    "部品棚 柱4": (5800.0, -5700.0),
    "検査台": (5200.0, -3600.0),
    "検査カメラ": (5200.0, -3600.0),
    "カメラ支柱": (5200.0, -3600.0),
    "出荷パレット": (7200.0, -1900.0),
    "充電ステーション": (-2400.0, -1100.0),
}


def moved(node, shift):
    node = copy.deepcopy(node)
    dx, dz = shift
    kind = node["kind"]
    if kind["type"] == "Box":
        kind["position"][0] += dx
        kind["position"][2] += dz
    else:
        node["position"][0] += dx
        node["position"][2] += dz
    return node


def regroup(group, shifts):
    """デモラインのアセンブリから、shifts にある部品だけを置き直して並べ直す"""
    by_name = {c["name"]: c for c in group["children"]}
    out = copy.deepcopy(group)
    out["children"] = [moved(by_name[name], shift) for name, shift in shifts.items()]
    return out


# ---------------------------------------------------------------------------
# 床: 安全機器・作業者・AGV
# ---------------------------------------------------------------------------

FENCE_H = 1800.0
X_BACK, X_FRONT = -900.0, 1300.0  # ロボットの後ろと、通路側
Z_IN, Z_OUT = -1150.0, 1350.0  # 供給側と排出側 (コンベアは柵をくぐる)


def device(id_, name, kind, position, yaw, size, stop_time=0.2):
    return {
        "id": id_,
        "name": name,
        "kind": kind,
        "position": position,
        "yaw_deg": yaw,
        "size": size,
        "stop_time": stop_time,
        "resolution": 30.0,
        "required_clearance": 850.0,
    }


def fence(id_, name, x0, z0, x1, z1):
    """(x0, z0) から (x1, z1) までの柵。X か Z のどちらかにまっすぐ置く"""
    if z0 == z1:
        return device(id_, name, "Fence", [(x0 + x1) / 2, 0.0, z0], 0.0, [abs(x1 - x0), FENCE_H, 50.0])
    return device(id_, name, "Fence", [x0, 0.0, (z0 + z1) / 2], 90.0, [abs(z1 - z0), FENCE_H, 50.0])


DOOR = (-250.0, 450.0)  # 通路側の扉 (ライトカーテン) の Z の範囲

SAFETY = [
    fence(1, "安全柵 (奥)", X_BACK, Z_IN, X_BACK, Z_OUT),
    fence(2, "安全柵 (供給側)", X_BACK, Z_IN, X_FRONT, Z_IN),
    fence(3, "安全柵 (排出側)", X_BACK, Z_OUT, X_FRONT, Z_OUT),
    fence(4, "安全柵 (手前 供給側)", X_FRONT, Z_IN, X_FRONT, DOOR[0]),
    fence(5, "安全柵 (手前 排出側)", X_FRONT, DOOR[1], X_FRONT, Z_OUT),
    device(6, "ライトカーテン (扉)", "LightCurtain", [X_FRONT, 300.0, sum(DOOR) / 2], 90.0,
           [DOOR[1] - DOOR[0], 1500.0, 30.0], stop_time=0.25),
    device(7, "非常停止 (操作台)", "EmergencyStop", [1650.0, 0.0, -2300.0], 0.0, [80.0, 1100.0, 80.0]),
    device(8, "エリアセンサ (保守口)", "AreaSensor", [-1500.0, 0.0, 100.0], 0.0, [1000.0, 300.0, 2000.0]),
]

OPERATORS = [
    {
        "id": 1,
        "name": "投入担当",
        "start": [1300.0, -2300.0],
        "walk_speed": 1100.0,
        "tasks": [
            {"type": "Walk", "to": [1500.0, -2700.0]},
            {"type": "Pick", "label": "部品棚から取る", "duration": 3.0},
            {"type": "Walk", "to": [950.0, -1480.0]},
            {"type": "Pick", "label": "シュートに投入", "duration": 2.5},
            {"type": "Walk", "to": [1650.0, -2100.0]},
            {"type": "Button", "label": "起動", "duration": 1.0},
            {"type": "Walk", "to": [1300.0, -2300.0]},
        ],
    },
    {
        "id": 2,
        "name": "検査担当",
        "start": [2800.0, 1300.0],
        "walk_speed": 1000.0,
        "tasks": [
            {"type": "Walk", "to": [2650.0, 800.0]},
            {"type": "Pick", "label": "AGV から受け取る", "duration": 3.0},
            {"type": "Walk", "to": [2800.0, 900.0]},
            {"type": "Button", "label": "検査", "duration": 6.0},
            {"type": "Walk", "to": [3400.0, 2000.0]},
            {"type": "Pick", "label": "パレットに積む", "duration": 3.0},
            {"type": "Walk", "to": [2800.0, 1300.0]},
        ],
    },
]

AGV_X = 2200.0  # AGV の通り道 (通路側の柵と検査台の間)

AGV = {
    "nodes": [
        {"id": 1, "name": "セル出口", "pos": [900.0, 2300.0], "station": True},
        {"id": 2, "name": "交差点", "pos": [AGV_X, 2300.0], "station": False},
        {"id": 3, "name": "検査場", "pos": [AGV_X, 800.0], "station": True},
        {"id": 5, "name": "部品供給", "pos": [AGV_X, -2800.0], "station": True},
        {"id": 6, "name": "充電", "pos": [AGV_X, 3800.0], "station": True},
    ],
    "edges": [[1, 2], [2, 3], [3, 5], [2, 6]],
    "flows": [{"from": 1, "to": 3, "interval": 40.0}, {"from": 5, "to": 1, "interval": 90.0}],
    "count": 2,
    "speed": 800.0,
    "load_time": 4.0,
    "unload_time": 4.0,
    "length": 900.0,
    "width": 600.0,
    "gap": 300.0,
}


def flow(demo_flow, cell_interval):
    """デモラインの工程モデルを、このラインの部品の番号とサイクルに合わせる"""
    f = copy.deepcopy(demo_flow)
    site = list(SITE_SHIFT)
    parts = {
        "部品投入": [1, 3],  # セル / 投入シュート
        "ロボット移載": [0],  # ロボット (UR5e + 2F-85)
        "排出コンベア": [1, 2],  # セル / 排出コンベア
        "外観検査": [3, site.index("検査台")],
        "出荷": [3, site.index("出荷パレット")],
    }
    for e in f["elements"]:
        e["part"] = parts[e["name"]]
        if e["name"] == "部品投入":
            e["interval"] = round(cell_interval + 0.4, 2)
        if e["name"] == "ロボット移載":
            e["cycle_time"] = cell_interval
    return f


def main():
    cell = json.loads(CELL.read_text(encoding="utf-8"))
    demo = json.loads(DEMO.read_text(encoding="utf-8"))
    panel = next(n for n in demo["nodes"] if n["name"] == "制御盤まわり")
    site = next(n for n in demo["nodes"] if n["name"] == "周辺設備")

    nodes = cell["nodes"] + [regroup(panel, PANEL_SHIFT), regroup(site, SITE_SHIFT)]
    assert [n["name"] for n in nodes[:2]] == ["ロボット (UR5e + 2F-85)", "セル"]
    interval = cell["conveyors"]["sources"][0]["interval"]

    out = {
        "format_version": cell["format_version"],
        "unit": "mm",
        "step_files": cell["step_files"],
        "nodes": nodes,
        "flow": flow(demo["flow"], interval),
        "floor": {"safety": SAFETY, "operators": OPERATORS, "agv": AGV},
        "chart": cell["chart"],
        "conveyors": cell["conveyors"],
    }
    OUT.write_text(json.dumps(out, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"{OUT.relative_to(ROOT)} を書きました")


if __name__ == "__main__":
    main()
