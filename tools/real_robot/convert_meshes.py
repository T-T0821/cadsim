#!/usr/bin/env python3
"""メーカー公開の ROS 記述パッケージのメッシュ (COLLADA) を、cadsim で開ける glTF (GLB) にまとめる。

    git clone https://github.com/UniversalRobots/Universal_Robots_ROS2_Description.git ur
    git clone https://github.com/PickNikRobotics/ros2_robotiq_gripper.git robotiq
    pip install trimesh pycollada
    python3 tools/real_robot/convert_meshes.py ur robotiq   # samples/real_robot/ に書く

- ur5e.glb: UR5e の 7 リンク。ホーム姿勢 (kinematics.UR5E_HOME) に組んだ形で、原点はベースの底面の中心
- robotiq_2f85.glb: Robotiq 2F-85 と UR 用アダプタ。全開の形で、原点はアダプタの取り付け面 (UR の tool0)

どちらも Y が上、単位は m (glTF の決まり)。リンクごとのノードの下に、色ごとのメッシュを子として置きます。
cadsim の関節はリンクのノードを動かすので、ノード名 (= 製品名) はファイルの中で重ならないようにしています。
"""

import json
import struct
import sys
from pathlib import Path

import numpy as np
import trimesh

sys.path.insert(0, str(Path(__file__).resolve().parent))
import kinematics as k  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "samples" / "real_robot"

# 元の COLLADA の色 (sRGB 0..255) → 名前
COLOR_NAMES = {
    (71, 71, 71): "gray",
    (8, 8, 8): "black",
    (125, 173, 204): "blue",
    (209, 209, 209): "silver",
    (204, 204, 204): "silver",
    (76, 76, 76): "gray",
    (10, 10, 10): "black",
}


def srgb_to_linear(c):
    c = c / 255.0
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def load_parts(path, transform):
    """COLLADA を読み、色ごとに頂点を transform で写した三角形メッシュにまとめる"""
    scene = trimesh.load(path, force="scene")
    by_color = {}
    for node in scene.graph.nodes_geometry:
        node_tf, geom_name = scene.graph[node]
        geom = scene.geometry[geom_name]
        color = tuple(int(v) for v in geom.visual.material.main_color[:3])
        mesh = geom.copy()
        mesh.apply_transform(np.asarray(transform) @ node_tf)
        by_color.setdefault(color, []).append(mesh)
    parts = []
    for color, meshes in sorted(by_color.items(), key=lambda kv: COLOR_NAMES.get(kv[0], str(kv[0]))):
        mesh = trimesh.util.concatenate(meshes)
        mesh.merge_vertices()
        parts.append((COLOR_NAMES.get(color, "c%02x%02x%02x" % color), color, mesh))
    return parts


class GlbWriter:
    """位置・インデックス・単色のマテリアルだけの最小の GLB"""

    def __init__(self):
        self.gltf = {
            "asset": {"version": "2.0", "generator": "cadsim tools/real_robot/convert_meshes.py"},
            "scene": 0,
            "scenes": [{"nodes": []}],
            "nodes": [],
            "meshes": [],
            "materials": [],
            "accessors": [],
            "bufferViews": [],
            "buffers": [],
        }
        self.bin = bytearray()
        self.material_of = {}

    def _view(self, data, target):
        while len(self.bin) % 4:
            self.bin.append(0)
        offset = len(self.bin)
        self.bin.extend(data)
        self.gltf["bufferViews"].append(
            {"buffer": 0, "byteOffset": offset, "byteLength": len(data), "target": target}
        )
        return len(self.gltf["bufferViews"]) - 1

    def _material(self, color):
        if color not in self.material_of:
            self.gltf["materials"].append(
                {
                    "name": COLOR_NAMES.get(color, "color"),
                    "pbrMetallicRoughness": {
                        "baseColorFactor": [round(srgb_to_linear(c), 5) for c in color] + [1.0],
                        "metallicFactor": 0.1,
                        "roughnessFactor": 0.6,
                    },
                }
            )
            self.material_of[color] = len(self.gltf["materials"]) - 1
        return self.material_of[color]

    def mesh(self, name, color, mesh):
        positions = np.asarray(mesh.vertices, dtype=np.float32)
        indices = np.asarray(mesh.faces, dtype=np.uint32).reshape(-1)
        pos_view = self._view(positions.tobytes(), 34962)
        idx_view = self._view(indices.tobytes(), 34963)
        acc = self.gltf["accessors"]
        acc.append(
            {
                "bufferView": pos_view,
                "componentType": 5126,
                "count": len(positions),
                "type": "VEC3",
                "min": positions.min(axis=0).tolist(),
                "max": positions.max(axis=0).tolist(),
            }
        )
        acc.append({"bufferView": idx_view, "componentType": 5125, "count": len(indices), "type": "SCALAR"})
        self.gltf["meshes"].append(
            {
                "name": name,
                "primitives": [
                    {"attributes": {"POSITION": len(acc) - 2}, "indices": len(acc) - 1, "material": self._material(color)}
                ],
            }
        )
        return len(self.gltf["meshes"]) - 1

    def node(self, name, children=(), mesh=None):
        node = {"name": name}
        if children:
            node["children"] = list(children)
        if mesh is not None:
            node["mesh"] = mesh
        self.gltf["nodes"].append(node)
        return len(self.gltf["nodes"]) - 1

    def write(self, path, roots):
        self.gltf["scenes"][0]["nodes"] = roots
        while len(self.bin) % 4:
            self.bin.append(0)
        self.gltf["buffers"] = [{"byteLength": len(self.bin)}]
        js = json.dumps(self.gltf, separators=(",", ":")).encode()
        js += b" " * (-len(js) % 4)
        total = 12 + 8 + len(js) + 8 + len(self.bin)
        with open(path, "wb") as f:
            f.write(struct.pack("<III", 0x46546C67, 2, total))
            f.write(struct.pack("<II", len(js), 0x4E4F534A))
            f.write(js)
            f.write(struct.pack("<II", len(self.bin), 0x004E4942))
            f.write(self.bin)


def write_file(path, root_name, links):
    """links: [(リンク名, [(色名, 色, メッシュ)])]"""
    w = GlbWriter()
    link_nodes = []
    for link, parts in links:
        children = [w.node(f"{link}_{cname}", mesh=w.mesh(f"{link}_{cname}", color, m)) for cname, color, m in parts]
        link_nodes.append(w.node(link, children))
    root = w.node(root_name, link_nodes)
    w.write(path, [root])
    tris = sum(len(m.faces) for _, parts in links for _, _, m in parts)
    print(f"{path}: {len(links)} リンク, {tris} 三角形, {path.stat().st_size / 1e6:.1f} MB")


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(1)
    ur = Path(sys.argv[1])
    robotiq = Path(sys.argv[2])
    OUT.mkdir(parents=True, exist_ok=True)

    frames = k.ur5e_frames(k.UR5E_HOME)
    links = []
    for link in k.UR5E_LINKS:
        tf = k.chain(k.ZUP_TO_YUP, frames[link], k.UR5E_MESH_OFFSETS[link])
        dae = ur / "meshes" / "ur5e" / "visual" / f"{k.UR5E_MESH_FILES[link]}.dae"
        links.append((link, load_parts(dae, tf)))
    write_file(OUT / "ur5e.glb", "UR5e", links)

    gframes = k.gripper_frames(0.0)
    links = []
    for link, *_ in k.GRIPPER_LINKS:
        tf = k.chain(k.ZUP_TO_YUP, gframes[link])
        dae = robotiq / "robotiq_description" / "meshes" / "visual" / "2f_85" / f"{link}.dae"
        links.append((link, load_parts(dae, tf)))
    write_file(OUT / "robotiq_2f85.glb", "Robotiq 2F-85", links)


if __name__ == "__main__":
    main()
