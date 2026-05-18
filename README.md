# pipeview

`pipeview` is a Rust terminal tool for visualizing configurable pipeline logs.

`pipeview` 是一个使用Rust编写的终端流水线可视化工具.

## Usage

Open a PLog file in the terminal timeline view:

在终端时间线视图中打开 PLog 文件:

```bash
cargo run -- examples/classic_5stage_bottleneck.plog
```

Print a text report:

输出文本报告:

```bash
cargo run -- report examples/classic_5stage_bottleneck.plog
```

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
| `Shift + mouse wheel` | Move the cycle window |
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
| `Shift + 鼠标滚轮` | 移动 cycle 窗口 |
| `Ctrl + 鼠标滚轮` | 在终端支持时缩放 |

## License

BSD 3-Clause. See [LICENSE](LICENSE).
