"""UR5e と Robotiq 2F-85 の関節の位置 (標準ライブラリだけで計算する)。

寸法はメーカーが公開している ROS の記述パッケージの値をそのまま写しています。

- UR5e: Universal_Robots_ROS2_Description の config/ur5e/default_kinematics.yaml と
  visual_parameters.yaml、urdf/ur_macro.xacro
- 2F-85: ros2_robotiq_gripper の robotiq_description/urdf/robotiq_2f_85_macro.urdf.xacro と
  ur_to_robotiq_adapter.urdf.xacro

ROS の座標 (m、Z が上) で計算し、`ZUP_TO_YUP` で cadsim の座標 (Y が上) に写します。
convert_meshes.py (メッシュの書き出し) と make_demo.py (プロジェクトの生成) の両方から使います。
"""

import math

PI = math.pi

# ---------------------------------------------------------------------------
# 4x4 行列 (リストのリスト)
# ---------------------------------------------------------------------------


def identity():
    return [[1.0 if i == j else 0.0 for j in range(4)] for i in range(4)]


def mul(a, b):
    return [[sum(a[i][k] * b[k][j] for k in range(4)) for j in range(4)] for i in range(4)]


def chain(*ms):
    out = identity()
    for m in ms:
        out = mul(out, m)
    return out


def rpy(roll, pitch, yaw):
    """URDF の rpy (Rz(yaw) · Ry(pitch) · Rx(roll))"""
    cr, sr = math.cos(roll), math.sin(roll)
    cp, sp = math.cos(pitch), math.sin(pitch)
    cy, sy = math.cos(yaw), math.sin(yaw)
    return [
        [cy * cp, cy * sp * sr - sy * cr, cy * sp * cr + sy * sr, 0.0],
        [sy * cp, sy * sp * sr + cy * cr, sy * sp * cr - cy * sr, 0.0],
        [-sp, cp * sr, cp * cr, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]


def origin(xyz=(0.0, 0.0, 0.0), r=(0.0, 0.0, 0.0)):
    m = rpy(*r)
    for i in range(3):
        m[i][3] = xyz[i]
    return m


def rot_axis(axis, angle):
    """原点を通る軸まわりの回転"""
    x, y, z = axis
    c, s = math.cos(angle), math.sin(angle)
    t = 1.0 - c
    return [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y, 0.0],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x, 0.0],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]


def apply(m, p):
    return [sum(m[i][k] * p[k] for k in range(3)) + m[i][3] for i in range(3)]


def apply_dir(m, v):
    return [sum(m[i][k] * v[k] for k in range(3)) for i in range(3)]


def translation(m):
    return [m[0][3], m[1][3], m[2][3]]


# ROS (Z 上) → cadsim (Y 上)。(x, y, z) → (x, z, -y)
ZUP_TO_YUP = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, -1.0, 0.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
]
YUP_TO_ZUP = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 0.0, -1.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
]

# ---------------------------------------------------------------------------
# UR5e
# ---------------------------------------------------------------------------

UR5E_LINKS = ["base", "shoulder", "upper_arm", "forearm", "wrist_1", "wrist_2", "wrist_3"]
UR5E_JOINTS = [
    "shoulder_pan",
    "shoulder_lift",
    "elbow",
    "wrist_1",
    "wrist_2",
    "wrist_3",
]
# 関節 i (UR5E_JOINTS[i]) が動かすリンク = UR5E_LINKS[i + 1]。各関節は子リンクの Z 軸まわりに回る
UR5E_JOINT_ORIGINS = [
    origin((0.0, 0.0, 0.1625)),
    origin(r=(PI / 2, 0.0, 0.0)),
    origin((-0.425, 0.0, 0.0)),
    origin((-0.3922, 0.0, 0.1333)),
    origin((0.0, -0.0997, 0.0), (PI / 2, 0.0, 0.0)),
    origin((0.0, 0.0996, 0.0), (PI / 2, PI, PI)),
]
# 見た目のメッシュをリンクの座標に置く変換 (visual_parameters.yaml の mesh_offset)
UR5E_MESH_OFFSETS = {
    "base": origin(r=(0.0, 0.0, PI)),
    "shoulder": origin(r=(0.0, 0.0, PI)),
    "upper_arm": origin((0.0, 0.0, 0.138), (PI / 2, 0.0, -PI / 2)),
    "forearm": origin((0.0, 0.0, 0.007), (PI / 2, 0.0, -PI / 2)),
    "wrist_1": origin((0.0, 0.0, -0.127), (PI / 2, 0.0, 0.0)),
    "wrist_2": origin((0.0, 0.0, -0.0997)),
    "wrist_3": origin((0.0, -0.0005, -0.0989), (PI / 2, 0.0, 0.0)),
}
UR5E_MESH_FILES = {
    "base": "base",
    "shoulder": "shoulder",
    "upper_arm": "upperarm",
    "forearm": "forearm",
    "wrist_1": "wrist1",
    "wrist_2": "wrist2",
    "wrist_3": "wrist3",
}
# ペンダントの「ホーム」に近い姿勢 (腕を立てて工具を真下に向ける)。デモではこれを設計位置にする
UR5E_HOME = [0.0, -PI / 2, PI / 2, -PI / 2, -PI / 2, 0.0]


def ur5e_frames(q):
    """関節角 q (rad, 6 個) での各リンクと tool0 の変換 (base_link から見た ROS の座標)"""
    # base_link → base_link_inertia は Z まわりに 180°
    frame = origin(r=(0.0, 0.0, PI))
    frames = {"base": frame}
    for i, link in enumerate(UR5E_LINKS[1:]):
        frame = chain(frame, UR5E_JOINT_ORIGINS[i], rot_axis((0.0, 0.0, 1.0), q[i]))
        frames[link] = frame
    # wrist_3 → flange → tool0 (tool0 の Z が工具の向き)
    flange = chain(frames["wrist_3"], origin(r=(0.0, -PI / 2, -PI / 2)))
    frames["tool0"] = chain(flange, origin(r=(PI / 2, 0.0, PI / 2)))
    return frames


# ---------------------------------------------------------------------------
# Robotiq 2F-85 (UR 用アダプタ付き)。原点はアダプタの取り付け面 = UR の tool0
# ---------------------------------------------------------------------------

GRIPPER_BASE_Z = 0.011  # アダプタの厚さ
# (リンク, 親, 関節の位置, 開閉の角度に掛ける係数)。係数 0 は固定
GRIPPER_LINKS = [
    ("ur_to_robotiq_adapter", None, (0.0, 0.0, 0.0), 0),
    ("robotiq_base", "ur_to_robotiq_adapter", (0.0, 0.0, GRIPPER_BASE_Z), 0),
    ("left_knuckle", "robotiq_base", (0.03060114, 0.0, 0.05490452), 1),
    ("right_knuckle", "robotiq_base", (-0.03060114, 0.0, 0.05490452), -1),
    ("left_finger", "left_knuckle", (0.03152616, 0.0, -0.00376347), 0),
    ("right_finger", "right_knuckle", (-0.03152616, 0.0, -0.00376347), 0),
    ("left_inner_knuckle", "robotiq_base", (0.0127, 0.0, 0.06142), 1),
    ("right_inner_knuckle", "robotiq_base", (-0.0127, 0.0, 0.06142), -1),
    ("left_finger_tip", "left_finger", (0.00563134, 0.0, 0.04718515), -1),
    ("right_finger_tip", "right_finger", (-0.00563134, 0.0, 0.04718515), 1),
]
GRIPPER_JOINT_AXIS = (0.0, -1.0, 0.0)
# 指先の内側の面 (指先リンクの座標)。開ききった (角度 0) とき左右の間が 85 mm になる位置
FINGER_PAD_X = 0.0
GRIPPER_STROKE = 0.085
GRIPPER_MAX_ANGLE = 0.8


def gripper_frames(angle):
    """開閉の角度 (rad, 0 で全開) での各リンクの変換 (アダプタの取り付け面から見た ROS の座標)"""
    frames = {}
    for link, parent, xyz, factor in GRIPPER_LINKS:
        base = frames[parent] if parent else identity()
        frames[link] = chain(base, origin(xyz), rot_axis(GRIPPER_JOINT_AXIS, factor * angle))
    return frames


def gripper_opening(angle):
    """開閉の角度での指先の間隔 (m)。指先の関節の位置の動きから求める"""
    x0 = translation(gripper_frames(0.0)["left_finger_tip"])[0]
    x = translation(gripper_frames(angle)["left_finger_tip"])[0]
    return GRIPPER_STROKE - 2.0 * (x0 - x)


def gripper_angle_for(width):
    """指先の間隔が width (m) になる角度 (二分法)"""
    lo, hi = 0.0, GRIPPER_MAX_ANGLE
    for _ in range(60):
        mid = (lo + hi) / 2
        if gripper_opening(mid) > width:
            lo = mid
        else:
            hi = mid
    return (lo + hi) / 2
