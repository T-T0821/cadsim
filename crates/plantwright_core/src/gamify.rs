//! ゲーム風の UI の中身: チュートリアルの章とミッション、経験値とレベル、達成時の紙吹雪、ミッションの表示の設定
//!
//! Bevy・egui・ファイル形式に依存しない。アプリ側は毎フレーム [`Observation`] を作って
//! [`Progress::observe`] に渡し、返ってきた達成を画面に出すだけにする。

use crate::workspace::Tool;

/// チュートリアルの章。基本編は「見る → さわる → ラインを組む → 仕上げる」、
/// 応用編は「動きを付ける → ラインとして動かす → 使いこなす」の順にまとめる。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Chapter {
    Look,
    Touch,
    BuildLine,
    Finish,
    Motion,
    RunLine,
    Master,
}

/// 章のまとまり。基本編で主な操作をひと回りし、応用編で設備の動き・PLC・AI などを覚える。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Part {
    Basic,
    Advanced,
}

impl Part {
    pub const ALL: [Part; 2] = [Part::Basic, Part::Advanced];

    pub fn title(self) -> &'static str {
        match self {
            Part::Basic => "基本編",
            Part::Advanced => "応用編",
        }
    }

    /// ようこそ画面に出す説明
    pub fn summary(self) -> &'static str {
        match self {
            Part::Basic => "見る → さわる → ラインを組む → 保存する (5 分ほど)",
            Part::Advanced => "動きを付ける → PLC・ライン連携で動かす → AI・一人称・動画",
        }
    }

    /// このまとまりの章
    pub fn chapters(self) -> impl Iterator<Item = Chapter> {
        Chapter::ALL.into_iter().filter(move |c| c.part() == self)
    }
}

impl Chapter {
    pub const ALL: [Chapter; 7] = [
        Chapter::Look,
        Chapter::Touch,
        Chapter::BuildLine,
        Chapter::Finish,
        Chapter::Motion,
        Chapter::RunLine,
        Chapter::Master,
    ];

    pub fn part(self) -> Part {
        match self {
            Chapter::Look | Chapter::Touch | Chapter::BuildLine | Chapter::Finish => Part::Basic,
            Chapter::Motion | Chapter::RunLine | Chapter::Master => Part::Advanced,
        }
    }

    /// 1 始まりの章番号
    pub fn number(self) -> usize {
        Self::ALL.iter().position(|&c| c == self).unwrap_or(0) + 1
    }

    pub fn title(self) -> &'static str {
        match self {
            Chapter::Look => "動くラインを見てみよう",
            Chapter::Touch => "部品にさわってみよう",
            Chapter::BuildLine => "ラインを組んでみよう",
            Chapter::Finish => "仕上げて残そう",
            Chapter::Motion => "設備に動きを付けよう",
            Chapter::RunLine => "ラインとして動かそう",
            Chapter::Master => "もっと使いこなそう",
        }
    }

    /// この章のミッション (おすすめ順)
    pub fn missions(self) -> impl Iterator<Item = &'static Mission> {
        MISSIONS.iter().filter(move |m| m.chapter == self)
    }
}

/// 操作する場所。アプリ側はここを光らせたり矢印で指したりする。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Place {
    /// 3D ビューの中
    Viewport,
    /// 左のアセンブリツリー
    Tree,
    /// 画面下のタイミングチャート
    Chart,
    /// 上のメニューバー
    Menu,
    /// 画面左端のツールレール (道具のボタン)
    Rail,
    /// キーボード
    Keyboard,
}

/// チュートリアルのミッション。並び順がおすすめの進め方。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MissionId {
    SelectPart,
    OrbitView,
    PlayChart,
    SwitchProjection,
    EditPart,
    Undo,
    Measure,
    FindInterference,
    PlaceLibraryPart,
    RunFlow,
    Annotate,
    Save,
    AddActuator,
    AddStep,
    AddSensor,
    EmergencyStop,
    RunPlc,
    RunLineLink,
    ComposeLine,
    ExportLadder,
    WalkAround,
    ExportVideo,
}

pub struct Mission {
    pub id: MissionId,
    /// 保存ファイルで使う名前 (変えない)
    pub key: &'static str,
    pub chapter: Chapter,
    pub title: &'static str,
    /// 画面のどこで何をすればよいか (1行)
    pub hint: &'static str,
    /// チュートリアルの手順。上から順にやればクリアできる
    pub steps: &'static [&'static str],
    /// この操作が何の役に立つか
    pub why: &'static str,
    /// 操作する場所
    pub place: Place,
    /// 設備が動いている間はできない (ギズモでの編集は設計位置で止まっているときだけ)
    pub needs_stopped_line: bool,
    /// 使うキー (カードにキーの形で出す)
    pub keys: &'static [&'static str],
    /// 使う道具。カードに「開く」ボタンを出す
    pub tool: Option<Tool>,
    pub xp: u32,
}

pub const MISSIONS: &[Mission] = &[
    Mission {
        id: MissionId::SelectPart,
        key: "select_part",
        chapter: Chapter::Look,
        title: "部品をえらぼう",
        hint: "3D ビューの部品か、左のツリーの行をクリック",
        steps: &[
            "3D ビューで動いている部品をどれか1つクリックする",
            "えらんだ部品が明るく光り、左のツリーの行も色が付く",
        ],
        why: "部品をえらぶと、右のプロパティで名前や位置を見たり変えたりできます",
        place: Place::Viewport,
        needs_stopped_line: false,
        keys: &[],
        tool: None,
        xp: 50,
    },
    Mission {
        id: MissionId::OrbitView,
        key: "orbit_view",
        chapter: Chapter::Look,
        title: "ぐるっと見まわそう",
        hint: "Alt を押しながら左ドラッグ (右ドラッグでも見まわせる)",
        steps: &[
            "3D ビューの上で、Alt を押しながら左ボタンでドラッグする (右ボタンのドラッグでもよい)",
            "ホイールで近づく・離れる。Home で全体が入る位置に戻る",
        ],
        why: "いろいろな向きから見ると、部品の重なりやすき間に気づけます",
        place: Place::Viewport,
        needs_stopped_line: false,
        keys: &["Alt", "ドラッグ"],
        tool: None,
        xp: 50,
    },
    Mission {
        id: MissionId::PlayChart,
        key: "play_chart",
        chapter: Chapter::Look,
        title: "設備を止めて、また動かそう",
        hint: "下のタイミングチャートの「⏸ 一時停止」→「▶ 再生」",
        steps: &[
            "画面下のタイミングチャートの「⏸ 一時停止」を押して動きを止める",
            "もう一度「▶ 再生」を押す。横の「x1」で速さも変えられる",
        ],
        why: "設備の動きはタイミングチャートの手順どおりに再生されます。止めて1コマずつ確かめられます",
        place: Place::Chart,
        needs_stopped_line: false,
        keys: &[],
        tool: None,
        xp: 80,
    },
    Mission {
        id: MissionId::SwitchProjection,
        key: "switch_projection",
        chapter: Chapter::Look,
        title: "図面モードに切り替えよう",
        hint: "5 キーか、ツールバーの「平行」で平行投影に",
        steps: &[
            "3D ビューをクリックしてから 5 キーを押す (3D ビュー上のツールバーの「平行」でもよい)",
            "遠近感のない図面のような見え方になる。もう一度押すと元に戻る",
        ],
        why: "平行投影では奥の部品も同じ大きさで見えるので、寸法や並びを確かめやすくなります",
        place: Place::Keyboard,
        needs_stopped_line: false,
        keys: &["5"],
        tool: None,
        xp: 50,
    },
    Mission {
        id: MissionId::EditPart,
        key: "edit_part",
        chapter: Chapter::Touch,
        title: "部品を動かそう",
        hint: "部品をえらんで W を押し、出てきた矢印をドラッグ",
        steps: &[
            "画面下のタイミングチャートの「⏮ 先頭へ」で設備を止める (動いている間は部品を動かせない)",
            "部品をクリックしてえらび、W を押すと赤・緑・青の矢印 (ギズモ) が出る",
            "矢印をドラッグして部品をずらす。E で回転、R で拡大縮小",
        ],
        why: "配置を変えながら、設備どうしの距離や通り道を検討できます",
        place: Place::Viewport,
        needs_stopped_line: true,
        keys: &["W", "E", "R"],
        tool: None,
        xp: 80,
    },
    Mission {
        id: MissionId::Undo,
        key: "undo",
        chapter: Chapter::Touch,
        title: "時間を巻き戻そう",
        hint: "Ctrl+Z で元に戻す (Ctrl+Y でやり直し)",
        steps: &[
            "Ctrl+Z を押すと、さっき動かした部品が元の位置に戻る",
            "Ctrl+Y でやり直せる。「編集」メニューからもできる",
        ],
        why: "失敗しても戻せるので、思い切って試せます",
        place: Place::Keyboard,
        needs_stopped_line: false,
        keys: &["Ctrl", "Z"],
        tool: None,
        xp: 60,
    },
    Mission {
        id: MissionId::Measure,
        key: "measure",
        chapter: Chapter::Touch,
        title: "距離をはかろう",
        hint: "M を押して、部品の表面を2回クリック",
        steps: &[
            "3D ビューで M を押して計測を始める",
            "はかりたい2か所の表面を順にクリックする",
            "2点の距離が線と数字で出る。Esc で計測をやめる",
        ],
        why: "設備のすき間や、作業者の手が届く距離を確かめられます",
        place: Place::Viewport,
        needs_stopped_line: false,
        keys: &["M"],
        tool: None,
        xp: 80,
    },
    Mission {
        id: MissionId::FindInterference,
        key: "find_interference",
        chapter: Chapter::Touch,
        title: "ぶつかりを見つけよう",
        hint: "部品を動かして別の部品に重ねると、赤く光る",
        steps: &[
            "設備を止めたまま、部品をえらんで W を押す",
            "矢印をドラッグして、となりの部品に重なるまで動かす",
            "重なったところが赤く光る。左端の 💥 干渉 を開くと一覧が見られる",
        ],
        why: "設備を作る前に、ぶつかる配置や動きを画面の上で見つけられます",
        place: Place::Viewport,
        needs_stopped_line: true,
        keys: &["W"],
        tool: Some(Tool::Interference),
        xp: 100,
    },
    Mission {
        id: MissionId::PlaceLibraryPart,
        key: "place_library_part",
        chapter: Chapter::BuildLine,
        title: "標準部品を置こう",
        hint: "左端の 📦 部品 を開き、部品を 3D ビューへドラッグ",
        steps: &[
            "画面左端のツールレールの 📦 部品 を押して部品ライブラリを開く (カードの「開く」でもよい)",
            "シリンダやコンベアの行を 3D ビューへドラッグして離す (+ を押すと注視点に置く)",
            "置いた部品は右のプロパティでストロークや長さを変えられる",
        ],
        why: "シリンダ・コンベア・センサーなどを寸法つきですぐ置けるので、ラインの形をすばやく試せます",
        place: Place::Rail,
        needs_stopped_line: false,
        keys: &[],
        tool: Some(Tool::Library),
        xp: 100,
    },
    Mission {
        id: MissionId::RunFlow,
        key: "run_flow",
        chapter: Chapter::BuildLine,
        title: "ラインを早回ししよう",
        hint: "左端の 📊 工程 を開き、「サンプル工程」を入れて「▶ 実行」",
        steps: &[
            "画面左端のツールレールの 📊 工程 を押す (カードの「開く」でもよい)",
            "工程が空なら「サンプル工程」を押す",
            "「▶ 実行」を押すと、数時間分を早回しして稼働率やスループットが出る",
        ],
        why: "どの工程が詰まるか (ボトルネック) を、作る前に数字で確かめられます",
        place: Place::Rail,
        needs_stopped_line: false,
        keys: &[],
        tool: Some(Tool::Flow),
        xp: 120,
    },
    Mission {
        id: MissionId::Annotate,
        key: "annotate",
        chapter: Chapter::Finish,
        title: "寸法を書きこもう",
        hint: "左端の 📐 寸法 を開き、点間距離などを置く",
        steps: &[
            "画面左端のツールレールの 📐 寸法 を押す (カードの「開く」でもよい)",
            "「点間距離」を押してから、部品の表面を2か所クリックする",
            "寸法線が 3D の中に残り、プロジェクトと一緒に保存される",
        ],
        why: "寸法やメモを図に残すと、ほかの人と検討するときに伝わりやすくなります",
        place: Place::Rail,
        needs_stopped_line: false,
        keys: &[],
        tool: Some(Tool::Annotation),
        xp: 80,
    },
    Mission {
        id: MissionId::Save,
        key: "save",
        chapter: Chapter::Finish,
        title: "セーブしよう",
        hint: "Ctrl+S でプロジェクトを保存",
        steps: &[
            "Ctrl+S を押す (「ファイル → 保存」でもよい)",
            "保存する場所と名前を決める。部品・チャート・工程がまとめて .plantwright に入る",
        ],
        why: "保存したファイルを開けば、いつでも続きから作業できます",
        place: Place::Keyboard,
        needs_stopped_line: false,
        keys: &["Ctrl", "S"],
        tool: None,
        xp: 80,
    },
    Mission {
        id: MissionId::AddActuator,
        key: "add_actuator",
        chapter: Chapter::Motion,
        title: "部品に動きを付けよう",
        hint: "タイミングチャートの「+ アクチュエータ」で作り、選んだ部品を割り当てる",
        steps: &[
            "3D ビューかツリーで、動かしたい部品をクリックしてえらぶ",
            "下のタイミングチャートの「+ アクチュエータ」→「シリンダ」を押す",
            "右側の設定欄の「+ 選択中の「…」を追加」を押すと、その部品がシリンダの先で動くようになる",
        ],
        why: "部品にシリンダ・直動軸・回転テーブルの役を持たせると、チャートの手順どおりに動かせます",
        place: Place::Chart,
        needs_stopped_line: false,
        keys: &[],
        tool: None,
        xp: 100,
    },
    Mission {
        id: MissionId::AddStep,
        key: "add_step",
        chapter: Chapter::Motion,
        title: "動きの順番を足そう",
        hint: "タイミングチャートの「+ ステップ」を押し、セルをダブルクリック",
        steps: &[
            "タイミングチャートの「+ ステップ」を押すと、S1, S2, … の列が1つ増える",
            "増えた列で、動かしたいアクチュエータの行のセルをダブルクリックする (シリンダは 出 → 戻 → なし)",
            "▶ 再生で、足したステップの動きを確かめる。サイクルタイムは自動で計算される",
        ],
        why: "同じステップの動作は同時に始まり、終わると次のステップへ進みます。順番を並べるだけで装置の動きが組めます",
        place: Place::Chart,
        needs_stopped_line: false,
        keys: &[],
        tool: None,
        xp: 100,
    },
    Mission {
        id: MissionId::AddSensor,
        key: "add_sensor",
        chapter: Chapter::Motion,
        title: "センサーで待たせよう",
        hint: "タイミングチャートの「+ センサー」で光電センサーを置く",
        steps: &[
            "タイミングチャートの「+ センサー」→「光電センサー」を押す",
            "右側の設定欄で、センサーの位置・向き・範囲を決める",
            "センサーの行のセルをダブルクリックすると、そのステップが「ON待」になる (インターロック)",
        ],
        why: "ワークが来るまで次の動きを始めない、といった条件を付けて、実機と同じ待ち方を確かめられます",
        place: Place::Chart,
        needs_stopped_line: false,
        keys: &[],
        tool: None,
        xp: 100,
    },
    Mission {
        id: MissionId::EmergencyStop,
        key: "emergency_stop",
        chapter: Chapter::RunLine,
        title: "非常停止を押してみよう",
        hint: "左端の 🛡 安全 を開き、非常停止の「押す」かセンサーの「遮光」",
        steps: &[
            "画面左端のツールレールの 🛡 安全 を押す (カードの「開く」でもよい)",
            "一覧に非常停止がなければ「追加:」の「非常停止」を押して置く",
            "非常停止の「押す」(ライトカーテンなら「遮光」) で、設備が安全停止するのを見る。「戻す」→「リセット」で再開",
        ],
        why: "どの安全機器が、どの設備を止めるのかを、据え付ける前に確かめられます",
        place: Place::Rail,
        needs_stopped_line: false,
        keys: &[],
        tool: Some(Tool::Safety),
        xp: 100,
    },
    Mission {
        id: MissionId::RunPlc,
        key: "run_plc",
        chapter: Chapter::RunLine,
        title: "PLC で動かそう",
        hint: "左端の 🔌 PLC を開き、「内蔵の模擬PLC」で「▶ 運転」",
        steps: &[
            "画面左端のツールレールの 🔌 PLC を押す (カードの「開く」でもよい)",
            "モードを「内蔵の模擬PLC」にする",
            "「▶ 運転」を押すと、出端・戻端の信号を見ながらチャートのステップが順に進む。I/O モニタで信号の ON / OFF が見られる",
        ],
        why: "実機の据え付け前に、ラダーと同じ信号のやりとりで設備を動かして確かめられます (仮想立ち上げ)",
        place: Place::Rail,
        needs_stopped_line: false,
        keys: &[],
        tool: Some(Tool::Plc),
        xp: 120,
    },
    Mission {
        id: MissionId::RunLineLink,
        key: "run_line_link",
        chapter: Chapter::RunLine,
        title: "ライン全体を同じ時計で動かそう",
        hint: "左端の 🔗 連携 を開き、「サンプルのラインを置く」→「▶ 再生」",
        steps: &[
            "画面左端のツールレールの 🔗 連携 を押す (カードの「開く」でもよい)",
            "工程がなければ「サンプルのラインを置く」を押す",
            "「▶ 再生」で、工程・AGV・作業者・タイミングチャートがいっしょに動く。時間スライダーで巻き戻しもできる",
        ],
        why: "AGV の待ちや安全停止がスループットにどう効くかを、ライン全体で確かめられます",
        place: Place::Rail,
        needs_stopped_line: false,
        keys: &[],
        tool: Some(Tool::LineLink),
        xp: 120,
    },
    Mission {
        id: MissionId::ComposeLine,
        key: "compose_line",
        chapter: Chapter::Master,
        title: "言葉でラインを組もう",
        hint: "左端の 💬 言葉で を開き、文を書いて「下書きを作る」→「モデルとチャートに置く」",
        steps: &[
            "画面左端のツールレールの 💬 言葉で を押す (カードの「開く」でもよい)",
            "「コンベア2本とロボット1台で箱詰め」のように作りたいラインを書いて「下書きを作る」",
            "配置図とステップを確かめて「モデルとチャートに置く」。Ctrl+Z で戻せる",
        ],
        why: "ラインのたたき台を数秒で作れます。AI の設定がなくても、文のキーワードから組めます",
        place: Place::Rail,
        needs_stopped_line: false,
        keys: &[],
        tool: Some(Tool::Compose),
        xp: 120,
    },
    Mission {
        id: MissionId::ExportLadder,
        key: "export_ladder",
        chapter: Chapter::Master,
        title: "ラダーを書き出そう",
        hint: "左端の ✨ ラダー を開き、「ラダーを書き出す」タブで保存かコピー",
        steps: &[
            "画面左端のツールレールの ✨ ラダー を押す (カードの「開く」でもよい)",
            "「ラダーを書き出す」タブを開く。今のチャートからステップラダーと I/O 表ができている",
            "「I/O 表をコピー」か「命令リストを保存…」を押す",
        ],
        why: "チャートから三菱 PLC 向けのラダーと I/O 表を作れるので、PLC のプログラムの下書きに使えます",
        place: Place::Rail,
        needs_stopped_line: false,
        keys: &[],
        tool: Some(Tool::SequenceGen),
        xp: 100,
    },
    Mission {
        id: MissionId::WalkAround,
        key: "walk_around",
        chapter: Chapter::Master,
        title: "工場の中を歩こう",
        hint: "ツールバーの「🎥 ビュー」→「🚶 一人称で歩く」",
        steps: &[
            "3D ビューの上のツールバーの「🎥 ビュー」→「🚶 一人称で歩く」を押す (「表示」メニューからでもよい)",
            "W A S D で歩き、マウスで向きを変える。左上に通路幅や手が届く部品が出る",
            "Esc で歩くのをやめる",
        ],
        why: "作業者の目の高さで、通路の幅・手の届く範囲・見通しを確かめられます",
        place: Place::Menu,
        needs_stopped_line: false,
        keys: &["W", "A", "S", "D"],
        tool: None,
        xp: 100,
    },
    Mission {
        id: MissionId::ExportVideo,
        key: "export_video",
        chapter: Chapter::Master,
        title: "動画にして見せよう",
        hint: "「ファイル → 書き出し → シミュレーションを動画で書き出し…」",
        steps: &[
            "メニューの「ファイル → 書き出し → シミュレーションを動画で書き出し…」を開く",
            "形式 (MP4・GIF) とサイクル数を選んで「書き出し開始…」を押し、保存先を決める",
            "撮り終わるまで待つ (Esc で中止)。打ち合わせの資料にそのまま貼れる",
        ],
        why: "動きを見せると、図面だけより早く意図が伝わります",
        place: Place::Menu,
        needs_stopped_line: false,
        keys: &[],
        tool: None,
        xp: 120,
    },
];

pub fn mission(id: MissionId) -> &'static Mission {
    MISSIONS
        .iter()
        .find(|m| m.id == id)
        .expect("すべての MissionId は MISSIONS にある")
}

/// 1レベル上がるのに要る経験値
pub const XP_PER_LEVEL: u32 = 150;

/// 見まわしたとみなすカメラの回転量 (ラジアン、ヨーとピッチの変化の合計)
const ORBIT_THRESHOLD: f64 = 0.6;

/// その時点の画面の状態。アプリ側が毎フレーム集める。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Observation {
    pub selected: bool,
    /// カメラのヨーとピッチ (ラジアン)
    pub camera_angles: [f64; 2],
    /// 3D ビューの上でマウスのボタンを押している (カメラの自動の動きと区別する)
    pub dragging_view: bool,
    pub orthographic: bool,
    /// いちばん新しい「元に戻す」の名前
    pub last_undo_label: Option<String>,
    pub can_redo: bool,
    pub measurement_count: usize,
    /// 干渉している部品の組の数
    pub interfering_pairs: usize,
    pub annotation_count: usize,
    pub playing: bool,
    /// ファイルに結びついたプロジェクトで、未保存の変更がない
    pub saved_to_file: bool,
    /// 標準部品ライブラリから置いた部品の数
    pub library_part_count: usize,
    /// 工程シミュレーションの結果が出ている
    pub flow_has_result: bool,
    /// タイミングチャートのアクチュエータに割り当てた部品の数 (全アクチュエータの合計)
    pub actuated_part_count: usize,
    /// タイミングチャートのステップの数
    pub step_count: usize,
    /// タイミングチャートのセンサーの数
    pub sensor_count: usize,
    /// 安全パネルで非常停止を押しているか、センサーを遮光している
    pub safety_tripped: bool,
    /// 内蔵の模擬 PLC で運転している
    pub plc_running: bool,
    /// ライン連携でラインが動いている
    pub line_running: bool,
    /// 「言葉でラインを組む」でモデルに置いた回数
    pub composed_count: u32,
    /// ラダーか I/O 表を書き出した (コピーした) 回数
    pub ladder_export_count: u32,
    /// 一人称で歩いている
    pub walking: bool,
    /// 動画を書き出し終えた回数
    pub video_export_count: u32,
}

/// ミッションを1つ達成したときの知らせ
#[derive(Clone, Debug, PartialEq)]
pub struct Achievement {
    pub mission: MissionId,
    pub xp: u32,
    /// レベルが上がったら、新しいレベル
    pub level_up: Option<u32>,
    /// これで章のミッションが全部終わったら、その章
    pub chapter_complete: Option<Chapter>,
    /// これで基本編・応用編のミッションが全部終わったら、そのまとまり
    pub part_complete: Option<Part>,
    /// これで全部のミッションが終わった
    pub all_complete: bool,
}

/// レベルの表示用
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub level: u32,
    /// 今のレベルに入ってから得た経験値
    pub into_level: u32,
    pub needed: u32,
}

impl Level {
    pub fn of(xp: u32) -> Self {
        Self {
            level: 1 + xp / XP_PER_LEVEL,
            into_level: xp % XP_PER_LEVEL,
            needed: XP_PER_LEVEL,
        }
    }

    pub fn fraction(&self) -> f32 {
        self.into_level as f32 / self.needed as f32
    }

    /// レベルに応じた称号
    pub fn title(&self) -> &'static str {
        level_title(self.level)
    }
}

/// レベルに応じた称号
pub fn level_title(level: u32) -> &'static str {
    match level {
        1 => "見習いエンジニア",
        2 => "かけだし設計者",
        3 => "ライン設計者",
        4 => "ベテラン設計者",
        5 => "設備マイスター",
        _ => "伝説のマイスター",
    }
}

/// ミッションの進み具合と経験値
#[derive(Clone, Debug, Default)]
pub struct Progress {
    completed: Vec<MissionId>,
    /// 「とばす」を押したミッション。あとで達成すれば経験値がもらえる
    skipped: Vec<MissionId>,
    xp: u32,
    /// 直前の状態 (変化を見つけるのに使う)。最初の観測では何も達成しない。
    last: Option<Observation>,
    /// 見まわしたカメラの回転量の合計
    orbit_travel: f64,
    /// 起動してから部品を編集したか (最初からある干渉を数えないため)
    edited: bool,
    /// 一覧の「挑戦」でえらんだミッション。おすすめ順より先にこれを出す (保存しない)
    focus: Option<MissionId>,
}

impl Progress {
    /// 保存しておいた進み具合から作る。知らない名前は無視する。
    pub fn restore<'a>(keys: impl IntoIterator<Item = &'a str>, xp: u32) -> Self {
        Self {
            completed: ids_of(keys, &[]),
            xp,
            ..Default::default()
        }
    }

    /// とばしたミッションも戻す。達成済みのものは達成のままにする。
    pub fn with_skipped<'a>(mut self, keys: impl IntoIterator<Item = &'a str>) -> Self {
        self.skipped = ids_of(keys, &self.completed);
        self
    }

    pub fn completed_keys(&self) -> Vec<&'static str> {
        self.completed.iter().map(|&id| mission(id).key).collect()
    }

    pub fn skipped_keys(&self) -> Vec<&'static str> {
        self.skipped.iter().map(|&id| mission(id).key).collect()
    }

    pub fn xp(&self) -> u32 {
        self.xp
    }

    pub fn level(&self) -> Level {
        Level::of(self.xp)
    }

    pub fn is_done(&self, id: MissionId) -> bool {
        self.completed.contains(&id)
    }

    pub fn is_skipped(&self, id: MissionId) -> bool {
        self.skipped.contains(&id)
    }

    /// 達成したか、とばした
    fn is_settled(&self, id: MissionId) -> bool {
        self.is_done(id) || self.is_skipped(id)
    }

    pub fn done_count(&self) -> usize {
        self.completed.len()
    }

    /// 達成したかとばしたミッションの数 (「ミッション 3/12」の表示に使う)
    pub fn settled_count(&self) -> usize {
        self.completed.len() + self.skipped.len()
    }

    /// 全部のミッションを達成した (とばしたものは含めない)
    pub fn all_complete(&self) -> bool {
        self.completed.len() == MISSIONS.len()
    }

    /// 次に挑戦するミッション。一覧でえらんだものがあればそれ、
    /// なければおすすめ順で最初の、達成もとばしもしていないもの
    pub fn next(&self) -> Option<&'static Mission> {
        if let Some(id) = self.focus.filter(|&id| !self.is_done(id)) {
            return Some(mission(id));
        }
        MISSIONS.iter().find(|m| !self.is_settled(m.id))
    }

    /// 一覧でえらんだミッションに今すぐ挑戦する (とばしていたら候補に戻す)
    pub fn focus(&mut self, id: MissionId) {
        if self.is_done(id) {
            return;
        }
        self.skipped.retain(|&s| s != id);
        self.focus = Some(id);
    }

    /// 一覧でえらんで挑戦しているミッション
    pub fn focused(&self) -> Option<MissionId> {
        self.focus.filter(|&id| !self.is_done(id))
    }

    /// 章の (達成した数, ミッションの数)
    pub fn chapter_progress(&self, chapter: Chapter) -> (usize, usize) {
        let total = chapter.missions().count();
        let done = chapter.missions().filter(|m| self.is_done(m.id)).count();
        (done, total)
    }

    /// 今のミッションをとばして次へ進む。経験値はもらえない。
    pub fn skip(&mut self, id: MissionId) {
        if self.focus == Some(id) {
            self.focus = None;
        }
        if !self.is_settled(id) {
            self.skipped.push(id);
        }
    }

    /// とばしたミッションをもう一度「次のミッション」の候補に戻す
    pub fn retry(&mut self, id: MissionId) {
        self.skipped.retain(|&s| s != id);
    }

    /// 次の観測を基準にし直す。プロジェクトを開いたときなど、操作によらず画面が変わるときに呼ぶ
    /// (開いたファイルに寸法や標準部品があっても、ミッション達成にしない)。
    pub fn rebase(&mut self) {
        self.last = None;
        self.orbit_travel = 0.0;
        self.edited = false;
    }

    /// 最初からやり直す
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 新しい状態を見て、このフレームで達成したミッションを返す
    pub fn observe(&mut self, now: &Observation) -> Vec<Achievement> {
        let Some(before) = self.last.replace(now.clone()) else {
            return Vec::new();
        };
        if now.dragging_view && before.dragging_view {
            self.orbit_travel += (now.camera_angles[0] - before.camera_angles[0]).abs()
                + (now.camera_angles[1] - before.camera_angles[1]).abs();
        }

        let edited = now.last_undo_label != before.last_undo_label
            && now.last_undo_label.as_deref().is_some_and(is_edit_label);
        self.edited |= edited;
        let reached = [
            (MissionId::SelectPart, now.selected && !before.selected),
            (MissionId::OrbitView, self.orbit_travel > ORBIT_THRESHOLD),
            (MissionId::EditPart, edited),
            (MissionId::Undo, now.can_redo && !before.can_redo),
            (
                MissionId::SwitchProjection,
                now.orthographic != before.orthographic,
            ),
            (
                MissionId::Measure,
                now.measurement_count > before.measurement_count,
            ),
            (
                MissionId::FindInterference,
                self.edited && now.interfering_pairs > before.interfering_pairs,
            ),
            (
                MissionId::Annotate,
                now.annotation_count > before.annotation_count,
            ),
            (MissionId::PlayChart, now.playing && !before.playing),
            (MissionId::Save, now.saved_to_file && !before.saved_to_file),
            (
                MissionId::PlaceLibraryPart,
                now.library_part_count > before.library_part_count,
            ),
            (
                MissionId::RunFlow,
                now.flow_has_result && !before.flow_has_result,
            ),
            (
                MissionId::AddActuator,
                now.actuated_part_count > before.actuated_part_count,
            ),
            (MissionId::AddStep, now.step_count > before.step_count),
            (MissionId::AddSensor, now.sensor_count > before.sensor_count),
            (
                MissionId::EmergencyStop,
                now.safety_tripped && !before.safety_tripped,
            ),
            (MissionId::RunPlc, now.plc_running && !before.plc_running),
            (
                MissionId::RunLineLink,
                now.line_running && !before.line_running,
            ),
            (
                MissionId::ComposeLine,
                now.composed_count > before.composed_count,
            ),
            (
                MissionId::ExportLadder,
                now.ladder_export_count > before.ladder_export_count,
            ),
            (MissionId::WalkAround, now.walking && !before.walking),
            (
                MissionId::ExportVideo,
                now.video_export_count > before.video_export_count,
            ),
        ];

        let mut achievements = Vec::new();
        for (id, hit) in reached {
            if hit && !self.is_done(id) {
                achievements.push(self.complete(id));
            }
        }
        achievements
    }

    fn complete(&mut self, id: MissionId) -> Achievement {
        let before = self.level().level;
        let m = mission(id);
        self.skipped.retain(|&s| s != id);
        if self.focus == Some(id) {
            self.focus = None;
        }
        self.completed.push(id);
        self.xp += m.xp;
        let after = self.level().level;
        let (done, total) = self.chapter_progress(m.chapter);
        let part = m.chapter.part();
        let part_done = MISSIONS
            .iter()
            .filter(|other| other.chapter.part() == part)
            .all(|other| self.is_done(other.id));
        Achievement {
            mission: id,
            xp: m.xp,
            level_up: (after > before).then_some(after),
            chapter_complete: (done == total).then_some(m.chapter),
            part_complete: part_done.then_some(part),
            all_complete: self.all_complete(),
        }
    }
}

/// 保存名からミッションを引く。知らない名前・重複・`except` にあるものは除く。
fn ids_of<'a>(keys: impl IntoIterator<Item = &'a str>, except: &[MissionId]) -> Vec<MissionId> {
    let mut ids = Vec::new();
    for key in keys {
        if let Some(m) = MISSIONS.iter().find(|m| m.key == key)
            && !ids.contains(&m.id)
            && !except.contains(&m.id)
        {
            ids.push(m.id);
        }
    }
    ids
}

/// 部品の編集・追加による履歴か (読み込みは数えない)。
/// 履歴の名前は `history` が「◯◯ を編集」「◯◯ を追加」の形で付ける。
fn is_edit_label(label: &str) -> bool {
    label.ends_with("を編集") || label.ends_with("を追加")
}

/// ミッションの表示と進み具合、画面の好み。アプリを閉じても覚えておく。
#[derive(Clone, Debug)]
pub struct Preferences {
    pub show_missions: bool,
    pub progress: Progress,
    /// 初回のようこそ画面を閉じた
    pub welcomed: bool,
    /// 3D ビューに浮かべた道具のパネルを、カーソルが乗っていない間は透かす
    pub see_through_panels: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            show_missions: true,
            progress: Progress::default(),
            welcomed: false,
            see_through_panels: true,
        }
    }
}

/// 紙吹雪の1片。位置は表示領域の幅・高さを 1 とした割合。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConfettiPiece {
    pub x0: f32,
    pub vx: f32,
    pub vy: f32,
    pub spin: f32,
    pub color: usize,
}

/// 紙吹雪の重力 (表示領域の高さ / 秒²)
const CONFETTI_GRAVITY: f32 = 1.4;

impl ConfettiPiece {
    /// 打ち上げてから `t` 秒後の位置 (x, y) と回転角
    pub fn at(&self, t: f32) -> (f32, f32, f32) {
        let x = self.x0 + self.vx * t;
        let y = self.vy * t + 0.5 * CONFETTI_GRAVITY * t * t;
        (x, y, self.spin * t)
    }
}

/// 上端から降らせる紙吹雪を `count` 片作る。同じ `seed` なら同じ並び。
pub fn confetti(seed: u64, count: usize, colors: usize) -> Vec<ConfettiPiece> {
    let mut state = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let mut next = || {
        // xorshift64*
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        (state.wrapping_mul(0x2545F4914F6CDD1D) >> 40) as f32 / (1u64 << 24) as f32
    };
    (0..count)
        .map(|_| ConfettiPiece {
            x0: next(),
            vx: (next() - 0.5) * 0.3,
            vy: -0.2 - next() * 0.4,
            spin: (next() - 0.5) * 12.0,
            color: (next() * colors.max(1) as f32) as usize % colors.max(1),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observe(progress: &mut Progress, edit: impl FnOnce(&mut Observation)) -> Vec<MissionId> {
        let mut now = progress.last.clone().unwrap_or_default();
        edit(&mut now);
        progress
            .observe(&now)
            .into_iter()
            .map(|a| a.mission)
            .collect()
    }

    #[test]
    fn first_observation_only_sets_baseline() {
        let mut progress = Progress::default();
        let start = Observation {
            selected: true,
            interfering_pairs: 3,
            ..Default::default()
        };
        assert!(progress.observe(&start).is_empty());
        assert_eq!(progress.done_count(), 0);
    }

    #[test]
    fn missions_complete_on_change() {
        let mut progress = Progress::default();
        observe(&mut progress, |_| {});
        assert_eq!(
            observe(&mut progress, |o| o.selected = true),
            [MissionId::SelectPart]
        );
        assert_eq!(
            observe(&mut progress, |o| o.measurement_count = 1),
            [MissionId::Measure]
        );
        assert_eq!(
            observe(&mut progress, |o| o.orthographic = true),
            [MissionId::SwitchProjection]
        );
        // 2回目は数えない
        assert!(observe(&mut progress, |o| o.selected = false).is_empty());
        assert!(observe(&mut progress, |o| o.selected = true).is_empty());
        assert_eq!(progress.xp(), 50 + 80 + 50);
    }

    #[test]
    fn loading_a_file_is_not_an_edit() {
        let mut progress = Progress::default();
        observe(&mut progress, |_| {});
        assert!(
            observe(&mut progress, |o| o.last_undo_label =
                Some("a.step を読み込み".into()))
            .is_empty()
        );
        assert_eq!(
            observe(&mut progress, |o| o.last_undo_label =
                Some("箱1 を編集".into())),
            [MissionId::EditPart]
        );
    }

    #[test]
    fn orbit_needs_enough_rotation() {
        let mut progress = Progress::default();
        observe(&mut progress, |_| {});
        // 全体表示などでカメラが自動で動いても数えない
        assert!(observe(&mut progress, |o| o.camera_angles = [2.0, 1.0]).is_empty());
        observe(&mut progress, |o| o.dragging_view = true);
        assert!(observe(&mut progress, |o| o.camera_angles = [2.3, 1.0]).is_empty());
        assert_eq!(
            observe(&mut progress, |o| o.camera_angles = [2.3, 1.4]),
            [MissionId::OrbitView]
        );
    }

    #[test]
    fn interference_counts_only_after_editing() {
        let mut progress = Progress::default();
        observe(&mut progress, |_| {});
        // 読み込んだ直後からある干渉は数えない
        assert!(observe(&mut progress, |o| o.interfering_pairs = 2).is_empty());
        assert_eq!(
            observe(&mut progress, |o| o.last_undo_label =
                Some("箱1 を編集".into())),
            [MissionId::EditPart]
        );
        assert_eq!(
            observe(&mut progress, |o| o.interfering_pairs = 3),
            [MissionId::FindInterference]
        );
    }

    #[test]
    fn level_up_and_all_complete() {
        let mut progress = Progress::default();
        let mut last = None;
        for m in MISSIONS {
            last = Some(progress.complete(m.id));
        }
        let last = last.unwrap();
        assert!(last.all_complete);
        assert!(progress.next().is_none());
        let total: u32 = MISSIONS.iter().map(|m| m.xp).sum();
        assert_eq!(progress.xp(), total);
        assert_eq!(progress.level().level, 1 + total / XP_PER_LEVEL);

        let mut fresh = Progress::default();
        let first = fresh.complete(MissionId::PlayChart);
        assert_eq!(first.level_up, None);
        let second = fresh.complete(MissionId::FindInterference);
        assert_eq!(second.level_up, Some(2));
    }

    #[test]
    fn restore_round_trip() {
        let mut progress = Progress::default();
        progress.complete(MissionId::Save);
        progress.complete(MissionId::Undo);
        let keys = progress.completed_keys();
        let restored = Progress::restore(keys.iter().copied().chain(["unknown"]), progress.xp());
        assert!(restored.is_done(MissionId::Save));
        assert!(restored.is_done(MissionId::Undo));
        assert_eq!(restored.done_count(), 2);
        assert_eq!(restored.next().unwrap().id, MissionId::SelectPart);
    }

    #[test]
    fn rebase_ignores_what_a_loaded_project_brings() {
        let mut progress = Progress::default();
        observe(&mut progress, |_| {});
        progress.rebase();
        // 開いたプロジェクトに寸法・標準部品があり、再生も始まった
        let loaded = Observation {
            annotation_count: 3,
            library_part_count: 5,
            playing: true,
            ..Default::default()
        };
        assert!(progress.observe(&loaded).is_empty());
        assert_eq!(
            observe(&mut progress, |o| o.annotation_count = 4),
            [MissionId::Annotate]
        );
    }

    #[test]
    fn skip_moves_on_and_can_still_be_achieved() {
        let mut progress = Progress::default();
        progress.skip(MissionId::SelectPart);
        assert_eq!(progress.next().unwrap().id, MissionId::OrbitView);
        assert_eq!(progress.settled_count(), 1);
        assert_eq!(progress.done_count(), 0);
        assert_eq!(progress.xp(), 0);
        // とばしたあとでも、やれば達成になって経験値がもらえる
        observe(&mut progress, |_| {});
        assert_eq!(
            observe(&mut progress, |o| o.selected = true),
            [MissionId::SelectPart]
        );
        assert!(!progress.is_skipped(MissionId::SelectPart));
        assert_eq!(progress.xp(), 50);

        progress.skip(MissionId::OrbitView);
        progress.retry(MissionId::OrbitView);
        assert_eq!(progress.next().unwrap().id, MissionId::OrbitView);
    }

    #[test]
    fn skipped_missions_are_not_all_complete() {
        let mut progress = Progress::default();
        for m in MISSIONS {
            progress.skip(m.id);
        }
        assert!(progress.next().is_none());
        assert!(!progress.all_complete());
    }

    #[test]
    fn skipped_round_trip() {
        let mut progress = Progress::default();
        progress.complete(MissionId::Save);
        progress.skip(MissionId::RunFlow);
        let restored = Progress::restore(progress.completed_keys(), progress.xp())
            .with_skipped(progress.skipped_keys().into_iter().chain(["save", "x"]));
        assert!(restored.is_skipped(MissionId::RunFlow));
        // 達成済みのものは、とばしたことにしない
        assert!(!restored.is_skipped(MissionId::Save));
        assert_eq!(restored.settled_count(), 2);
    }

    #[test]
    fn chapter_complete_is_reported_once() {
        let mut progress = Progress::default();
        let look: Vec<MissionId> = Chapter::Look.missions().map(|m| m.id).collect();
        let (last, rest) = look.split_last().unwrap();
        for &id in rest {
            assert_eq!(progress.complete(id).chapter_complete, None);
        }
        assert_eq!(
            progress.complete(*last).chapter_complete,
            Some(Chapter::Look)
        );
        assert_eq!(
            progress.chapter_progress(Chapter::Look),
            (look.len(), look.len())
        );
        assert_eq!(progress.chapter_progress(Chapter::Touch).0, 0);
    }

    #[test]
    fn every_chapter_has_missions_in_order() {
        // 章はミッション表の並びどおりに進む (章が行ったり来たりしない)
        let mut chapter = 0;
        for m in MISSIONS {
            let n = m.chapter.number();
            assert!(n >= chapter, "{} の章が前に戻っている", m.key);
            chapter = n;
            assert!(!m.steps.is_empty(), "{} に手順がない", m.key);
        }
        for c in Chapter::ALL {
            assert!(c.missions().count() > 0);
        }
    }

    #[test]
    fn library_and_flow_missions() {
        let mut progress = Progress::default();
        observe(&mut progress, |o| o.library_part_count = 2);
        assert_eq!(
            observe(&mut progress, |o| o.library_part_count = 3),
            [MissionId::PlaceLibraryPart]
        );
        assert_eq!(
            observe(&mut progress, |o| o.flow_has_result = true),
            [MissionId::RunFlow]
        );
    }

    #[test]
    fn advanced_missions_complete_on_change() {
        let mut progress = Progress::default();
        // 読み込んだ直後からある数や状態は数えない
        observe(&mut progress, |o| {
            o.actuated_part_count = 4;
            o.step_count = 6;
            o.sensor_count = 2;
        });
        type Edit = fn(&mut Observation);
        let cases: [(MissionId, Edit); 10] = [
            (MissionId::AddActuator, |o| o.actuated_part_count = 5),
            (MissionId::AddStep, |o| o.step_count = 7),
            (MissionId::AddSensor, |o| o.sensor_count = 3),
            (MissionId::EmergencyStop, |o| o.safety_tripped = true),
            (MissionId::RunPlc, |o| o.plc_running = true),
            (MissionId::RunLineLink, |o| o.line_running = true),
            (MissionId::ComposeLine, |o| o.composed_count = 1),
            (MissionId::ExportLadder, |o| o.ladder_export_count = 1),
            (MissionId::WalkAround, |o| o.walking = true),
            (MissionId::ExportVideo, |o| o.video_export_count = 1),
        ];
        for (id, edit) in cases {
            assert_eq!(observe(&mut progress, edit), [id]);
        }
        // 減っても、止めても達成にはならない
        assert!(
            observe(&mut progress, |o| {
                o.step_count = 1;
                o.walking = false;
                o.plc_running = false;
            })
            .is_empty()
        );
    }

    #[test]
    fn missions_with_a_rail_place_have_a_tool() {
        for m in MISSIONS {
            if m.place == Place::Rail {
                assert!(m.tool.is_some(), "{} の道具がない", m.key);
            }
        }
    }

    #[test]
    fn parts_split_chapters_basic_first() {
        let basic: Vec<Chapter> = Part::Basic.chapters().collect();
        let advanced: Vec<Chapter> = Part::Advanced.chapters().collect();
        assert_eq!(basic.len() + advanced.len(), Chapter::ALL.len());
        // 基本編の章が先に並ぶ
        let last_basic = basic.iter().map(|c| c.number()).max().unwrap();
        let first_advanced = advanced.iter().map(|c| c.number()).min().unwrap();
        assert!(last_basic < first_advanced);
        // 基本編だけを終えると、最後の1つで基本編クリアになり、応用編の最初のミッションが次になる
        let mut progress = Progress::default();
        let mut last = None;
        for m in MISSIONS.iter().filter(|m| m.chapter.part() == Part::Basic) {
            let achievement = progress.complete(m.id);
            if let Some(prev) = last.replace(achievement.part_complete) {
                assert_eq!(prev, None);
            }
        }
        assert_eq!(last, Some(Some(Part::Basic)));
        assert_eq!(progress.next().unwrap().chapter, advanced[0]);
        assert!(!progress.all_complete());
    }

    #[test]
    fn focus_jumps_to_a_chosen_mission() {
        let mut progress = Progress::default();
        progress.focus(MissionId::RunPlc);
        assert_eq!(progress.next().unwrap().id, MissionId::RunPlc);
        assert_eq!(progress.focused(), Some(MissionId::RunPlc));
        // 達成するとおすすめ順に戻る
        let done = progress.complete(MissionId::RunPlc);
        assert_eq!(done.mission, MissionId::RunPlc);
        assert_eq!(progress.focused(), None);
        assert_eq!(progress.next().unwrap().id, MissionId::SelectPart);

        // とばしていたミッションもえらべる。えらんだものをとばすとおすすめ順に戻る
        progress.skip(MissionId::Save);
        progress.focus(MissionId::Save);
        assert!(!progress.is_skipped(MissionId::Save));
        assert_eq!(progress.next().unwrap().id, MissionId::Save);
        progress.skip(MissionId::Save);
        assert_eq!(progress.next().unwrap().id, MissionId::SelectPart);

        // 達成済みのものはえらべない
        progress.focus(MissionId::RunPlc);
        assert_eq!(progress.focused(), None);
    }

    #[test]
    fn mission_keys_are_unique() {
        for (i, a) in MISSIONS.iter().enumerate() {
            for b in &MISSIONS[i + 1..] {
                assert_ne!(a.key, b.key);
                assert_ne!(a.id, b.id);
            }
        }
    }

    #[test]
    fn confetti_falls_and_is_deterministic() {
        let a = confetti(7, 40, 5);
        assert_eq!(a, confetti(7, 40, 5));
        assert_eq!(a.len(), 40);
        for piece in &a {
            assert!((0.0..1.0).contains(&piece.x0));
            assert!(piece.color < 5);
            // 最初は上に飛び、やがて下に落ちる
            assert!(piece.at(0.05).1 < 0.0);
            assert!(piece.at(3.0).1 > 1.0);
        }
    }
}
