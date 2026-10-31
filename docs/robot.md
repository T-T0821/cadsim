# 関節とロボット

タイミングチャートのアクチュエータ (回転テーブル・直動軸) を関節として扱い、
直列につないだものをロボットとして教示点へ動かすしくみです。コードは `src/motion/` にあります。

| ファイル | 層 | 内容 |
| --- | --- | --- |
| `chart.rs` | ドメイン | 関節の可動範囲、`Robot` / `TeachPoint` / `RobotMove`、移動を関節の動作に展開する `schedule` |
| `pose.rs` | ドメイン | 順運動学 (`actuator_poses_for`) |
| `kinematics.rs` | ドメイン | 関節の並び、TCP、ヤコビアン、逆運動学 |
| `robot_ui.rs` | UI | ロボット画面 (ジョグ・教示点) と 3D ビューへの表示 |

`chart.rs` / `pose.rs` / `kinematics.rs` は Bevy の型を使いません (ベクトルは `bevy::math` 経由の glam)。
[architecture.md](architecture.md) の移し方の 2 段目で `cadsim_core` の `domain::motion` に移します。

## データ

- **関節**: `Actuator` のうち `RotaryTable` (値は度) と `LinearAxis` (値はモデル単位)。
  `limits: Option<[f64; 2]>` が可動範囲で、再生 (`Schedule::value_at`) と逆運動学の両方で範囲内に収める。
  `Cylinder` は出 / 戻の 2 値なので逆運動学の対象外 (鎖の途中に載っていてもよく、その値のまま動かさない)。
- **ロボット**: `Robot { tip, tcp, tool_axis, points }`。関節は `tip` から `mounted_on` をたどった全アクチュエータ。
  `tcp` と `tool_axis` は設計位置 (全関節 0) でのモデル座標。
- **教示点**: `TeachPoint { position, approach }`。`approach` は工具軸を向けたい方向で、`None` なら位置だけ合わせる。
- **移動**: `Step::moves` の `RobotMove { robot, point, duration }`。

## 順運動学

各関節の動きは設計位置で定義した軸のまわり (直動なら軸に沿って) の剛体変換 M_i(q_i)。
関節 i の変換は「親の変換 ∘ M_i」で、根元から順に重ねる。これは指数積 (Product of Exponentials) の形で、
親の動きで子の軸が運ばれる。`pose::actuator_poses_for` が全アクチュエータについて計算する。

## 逆運動学

減衰最小二乗法 (Damped Least Squares)。

1. ヤコビアンの列: 関節 j の親の変換 P で軸 `a = P.R · dir` と回転中心 `p = P(pivot)` を求め、
   回転関節は 位置 `a × (tcp − p)`・角速度 `a`、直動関節は 位置 `a`・角速度 0 (回転はラジアンで扱う)。
2. 向きを指定したときは誤差に `tool_axis × approach` を足して 6 行で解く。
3. `Δq = Jᵀ (J Jᵀ + λ² I)⁻¹ e` を解き、可動範囲の端で外へ動こうとする関節は列を 0 にして解き直す。
4. 1 回の移動量に上限を付け、可動範囲に収めて繰り返す (最大 300 回)。

モデルの単位 (mm / m) によらず同じように解けるよう、ロボットの大きさ `reach`
(設計位置での回転中心から TCP までの最大距離) で、減衰係数・直動の移動量の上限・向きの行の重みをそろえる。

初期値 (直前の姿勢) から届かないときは、可動範囲の 1/2・1/4・3/4 などを初期値にして解き直し、
届いた解だけ採用する。どれでも届かなければ、初期値から近づけた姿勢と残りの誤差を返す
(位置 1e-3、向き約 0.5° 以内を「届いた」とする)。

## チャートへの展開

`TimingChart::schedule` はステップを順に見ながら各アクチュエータの直前の値を持っておき、
`RobotMove` があればその値を初期値に逆運動学を解いて、各関節に同じ動作時間の `Segment` を置く。
同じステップで関節に直接置いた動作とぶつかったときはロボットの移動を優先する。
結果は `Schedule::moves` (`SolvedMove`) に残り、UI の ⚠ 表示に使う。
関節を同時に補間する MoveJ なので、TCP の経路は直線にならない。

## ジョグ

`JogPose` (関節 id → 値) が空でない間、`update_pose` はチャートの値の代わりにこの値を使う。
再生位置が変わると `clear_jog_on_playback` が空に戻す。ジョグ中はギズモでの編集を止める (`at_design_pose`)。
