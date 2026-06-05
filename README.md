# pipeview

`pipeview` is a Rust terminal tool for visualizing configurable pipeline logs.

`pipeview` 是一个使用Rust编写的终端流水线可视化工具.

## Usage

Open a PLog file in the terminal timeline view:

在终端时间线视图中打开 PLog 文件:

```bash
cargo run -- examples/classic_5stage_bottleneck.plog
```

Open a large PLog by raising the uncompressed input limit. The default limit is
512 MiB and applies after `.zst` decompression:

提高未压缩输入大小上限后打开更大的 PLog. 默认上限是 512 MiB, 并且会作用在
`.zst` 解压后的文本大小上:

```bash
cargo run -- --max-input-mib 2048 examples/classic_5stage_bottleneck.plog.zst
```

Print a text report:

输出文本报告:

```bash
cargo run -- report examples/classic_5stage_bottleneck.plog
```

Compress a PLog file as `.zst` and remove the original PLog file:

将 PLog 文件压缩为 `.zst` 并删除原始 PLog 文件:

```bash
cargo run -- compress examples/classic_5stage_bottleneck.plog
```

Compressed `.zst` PLog files can be used directly with the TUI, `validate`,
and `report` commands:

压缩后的 `.zst` PLog 文件可以直接用于 TUI、`validate` 和 `report` 命令:

```bash
cargo run -- report examples/classic_5stage_bottleneck.plog.zst
```

## PLog Format

PLog is a tab-separated text format. Each file starts with `PLOG	1`. Fields
are separated by tabs, and optional attributes use `key=value`.

PLog 是一种使用 tab 分隔的文本格式. 每个文件以 `PLOG	1` 开头. 字段之间
使用 tab 分隔, 可选属性使用 `key=value`.

```text
PLOG	1
META	source	example
STAGE	IF	Fetch	order=10
STAGE	ID	Decode	order=20
LANE	main	Main	order=0
I	1	pc=0x80000000	inst=0x00000013
B	0	1	1	main	IF
B	1	2	1	main	ID	stall=decode
E	1	1	note	reason=hazard
C	1	fetch_queue	used=2
R	3	1	retire
```

Supported records:

支持的记录:

| Record | Fields | Meaning |
| --- | --- | --- |
| `PLOG` | `PLOG version` | File header. Version must be `1`. |
| `META` | `META key value` | File-level metadata. |
| `STAGE` | `STAGE id label [attrs...]` | Pipeline stage declaration. |
| `LANE` | `LANE id label [attrs...]` | Lane declaration. |
| `I` | `I inst_id [attrs...]` | Instruction declaration. |
| `B` | `B cycle duration inst_id lane stage [attrs...]` | Stage span. Duration is in cycles. |
| `E` | `E cycle inst_id event [attrs...]` | Per-instruction event. |
| `C` | `C cycle resource [attrs...]` | Counter/resource sample. |
| `R` | `R cycle inst_id status [attrs...]` | Retire or final instruction status. |

| 记录 | 字段 | 含义 |
| --- | --- | --- |
| `PLOG` | `PLOG version` | 文件头. 版本必须是 `1`. |
| `META` | `META key value` | 文件级元数据. |
| `STAGE` | `STAGE id label [attrs...]` | 流水级声明. |
| `LANE` | `LANE id label [attrs...]` | 通道声明. |
| `I` | `I inst_id [attrs...]` | 指令声明. |
| `B` | `B cycle duration inst_id lane stage [attrs...]` | 阶段 span. duration 单位是 cycle. |
| `E` | `E cycle inst_id event [attrs...]` | 指令事件. |
| `C` | `C cycle resource [attrs...]` | 计数器或资源采样. |
| `R` | `R cycle inst_id status [attrs...]` | 退休或最终指令状态. |

## Options

Global options are placed before the positional path or subcommand:

全局参数放在位置路径或子命令之前:

| Option | Meaning |
| --- | --- |
| `--no-color` | Disable stage colors in the TUI. |
| `--theme <PATH>` | Load a JSON stage color theme for the TUI. |
| `--max-input-mib <MIB>` | Maximum uncompressed PLog text to read. Default: `512`. |
| `-h`, `--help` | Print help. |
| `-V`, `--version` | Print version. |

| 参数 | 含义 |
| --- | --- |
| `--no-color` | 关闭 TUI 阶段配色. |
| `--theme <PATH>` | 为 TUI 加载 JSON 阶段配色主题. |
| `--max-input-mib <MIB>` | 最大可读取的未压缩 PLog 文本大小. 默认值: `512`. |
| `-h`, `--help` | 输出帮助. |
| `-V`, `--version` | 输出版本. |

Commands:

子命令:

| Command | Meaning |
| --- | --- |
| `pipeview <PATH>` | Open a PLog or `.zst` PLog in the TUI. |
| `pipeview validate <PATH>` | Validate a PLog or `.zst` PLog. |
| `pipeview report <PATH>` | Print a text summary. |
| `pipeview compress <PATH>` | Compress a plain PLog as `.zst` and delete the original file. |

| 子命令 | 含义 |
| --- | --- |
| `pipeview <PATH>` | 在 TUI 中打开 PLog 或 `.zst` PLog. |
| `pipeview validate <PATH>` | 检查 PLog 或 `.zst` PLog 是否有效. |
| `pipeview report <PATH>` | 输出文本报告. |
| `pipeview compress <PATH>` | 将普通 PLog 压缩为 `.zst` 并删除原文件. |

## Theme

The TUI colors stages by default. Stage colors are assigned by the `STAGE`
records in parsed order. A JSON theme can override stage colors:

TUI 默认会给阶段上色. 阶段颜色按照解析到的 `STAGE` 记录顺序分配. 可以使用
JSON 主题覆盖阶段颜色:

```bash
cargo run -- --theme examples/theme.json examples/classic_5stage_bottleneck.plog
```

Theme format:

主题格式:

```json
{
  "stages": {
    "IF": { "bg": "#ff8800", "fg": "white", "alpha": 1.0 },
    "ID": { "bg": "cyan", "fg": "black", "alpha": 1.0 },
    "EX": { "fg": "light_green", "alpha": 0.0 },
    "LS": "magenta"
  }
}
```

- `bg`: stage block background color.
- `fg`: stage label foreground color.
- `alpha`: `0.0` disables the background fill; `1.0` uses the background fill.
- A string value such as `"LS": "magenta"` means background color with white text.

- `bg`: 阶段色块背景色.
- `fg`: 阶段标签字体颜色.
- `alpha`: `0.0` 表示不填充背景; `1.0` 表示使用背景填充.
- 类似 `"LS": "magenta"` 的字符串写法表示使用该背景色和白色字体.

Disable colors:

关闭配色:

```bash
cargo run -- --no-color examples/classic_5stage_bottleneck.plog
```

## Keys

Arrow keys are the primary navigation controls: `Up` / `Down` move between
instruction rows, and `Left` / `Right` move the visible cycle window. Mouse
wheel shortcuts mirror those movements.

方向键是主要的导航方式: `Up` / `Down` 在指令行之间移动, `Left` / `Right`
移动可见 cycle 窗口. 鼠标滚轮快捷键对应这些移动方式.

| Key | Action |
| --- | --- |
| `q` | Quit |
| `Esc` | Close the current panel; quit when no panel is open |
| `Up` / `Down` | Move between instruction rows |
| `Left` / `Right` | Move the cycle window |
| `End` | Jump to the last occupied cycle in the selected row |
| `g` | Open jump input; enter `row,cycle` |
| `i` | Toggle the information panel |
| `d` | Toggle the selected instruction detail panel |
| `?` | Toggle the key help panel |
| `+` / `=` | Zoom in |
| `-` | Zoom out |
| `Mouse wheel` | Move between instruction rows |
| `Alt + mouse wheel` | Move the cycle window |
| `Ctrl + mouse wheel` | Zoom when supported by the terminal |

| 快捷键 | 功能 |
| --- | --- |
| `q` | 退出 |
| `Esc` | 关闭当前面板; 没有面板时退出 |
| `Up` / `Down` | 在指令行之间移动 |
| `Left` / `Right` | 移动 cycle 窗口 |
| `End` | 跳转到当前行最后一个有信息的 cycle |
| `g` | 打开跳转输入; 输入 `row,cycle` |
| `i` | 显示或隐藏信息面板 |
| `d` | 显示或隐藏当前选中指令详情面板 |
| `?` | 显示或隐藏快捷键帮助面板 |
| `+` / `=` | 放大 |
| `-` | 缩小 |
| `鼠标滚轮` | 在指令行之间移动 |
| `Alt + 鼠标滚轮` | 移动 cycle 窗口 |
| `Ctrl + 鼠标滚轮` | 在终端支持时缩放 |

## License

BSD 3-Clause. See [LICENSE](LICENSE).
