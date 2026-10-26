# 声音来源

全部是 CC0（公有领域），可以随意使用、修改、商用，不需要署名。

| 来源 | 作者 | 许可 | 用在 |
| --- | --- | --- | --- |
| [Sci-fi Sounds](https://kenney.nl/assets/sci-fi-sounds) | Kenney | CC0 | 武器、命中、爆炸、引擎 |
| [Interface Sounds](https://kenney.nl/assets/interface-sounds) | Kenney | CC0 | 界面、机库、锁定提示 |
| [Jet Engine](https://freesound.org/people/m_cel/sounds/534856/)（高音质预览版） | m_cel | CC0 | 喷气引擎 `engine.ogg` |
| [Afterburner](https://freesound.org/people/TiesWijnen/sounds/413312/)（高音质预览版） | TiesWijnen | CC0 | 加力燃烧 `burner.ogg`、`ignite.ogg` |
| [Cosmic Navigation](https://opengameart.org/content/cosmic-navigation)（循环版） | Synth-thetic | CC0 | 背景音乐 `music.ogg` |

## 哪个文件来自哪里

| 文件 | 原始素材 | 处理 |
| --- | --- | --- |
| `laser_0..4` | laserSmall_000..004 | — |
| `plasma_0..4` | laserLarge_000..004 | 短回声尾 |
| `rail_0..2` | laserRetro_000..002 | 降调到 0.7 倍，短回声尾 |
| `cannon_0..2` | explosionCrunch_000..002 | 升调到 1.45 倍，截到 0.32 秒 |
| `flak_0..1` | lowFrequency_explosion_001 + explosionCrunch_000 / 002 | 混合，截到 0.9 秒，短回声尾 |
| `missile_0..1` | thrusterFire_000..001 | 截取开头 0.9 秒，淡入淡出 |
| `hit_0..4` | impactMetal_000..004 | — |
| `shield_0..2` | forceField_000..002 | 截到 0.6 秒，淡出 |
| `pop_1..3` | explosionCrunch_001..003 | 截到 0.7 秒，淡出 |
| `boom_0..1` | lowFrequency_explosion_000 + explosionCrunch_004 / 003 | 混合，短回声尾 |
| `crash_1, crash_3` | impactMetal_001, 003 | 降调到 0.72 倍 |
| `confirm_hull` | impactMetal_002 + click_001 | 升调到 1.5 倍，截到 0.18 秒，叠一声点击 |
| `confirm_shield` | glass_003 | 截到 0.2 秒，淡出 |
| `confirm_kill` | confirmation_002 + lowFrequency_explosion_001 | 确认音叠在低频爆炸的前 0.4 秒上 |
| `engine` | Jet Engine（m_cel） | 取 0.5–9.5 秒的稳定运转段，末尾 1 秒和开头交叉淡化成 8 秒无缝循环 |
| `burner` | Afterburner（TiesWijnen） | 取 2.0–5.2 秒的稳定燃烧段，交叉淡化成 2.6 秒无缝循环 |
| `ignite` | Afterburner（TiesWijnen） | 开头 1.3 秒的点火，淡出 |
| `lock` / `warn` / `deny` | confirmation_001 / error_006 / error_008 | — |
| `place_1..4` / `erase` | drop_001..004 / close_001 | — |
| `wave` / `hover` / `click` | confirmation_004 / tick_001 / click_001 | — |
| `music` | cosmic_navigation_loop.flac | 转成 OGG（48 kHz 立体声），保持原曲的无缝循环 |

除了音乐，每个文件都把峰值归一到 -1 dBFS，游戏里各音效的响度在 `src/audio.rs` 的 `EFFECTS` 表里统一调。

## 重新生成

`make-sounds.sh` 从解压后的两个音效包、两段 Freesound 录音和音乐文件重新做出这里的全部文件，需要 ffmpeg，以及一个把 WAV 编码成 Ogg Vorbis 的程序（参数：输入 wav、输出 ogg、质量 0–1）：

```bash
./make-sounds.sh <Sci-fi Sounds/Audio> <Interface Sounds/Audio> cosmic_navigation_loop.flac 534856_7108357-hq.ogg 413312_1511977-hq.ogg . <编码器>
```
