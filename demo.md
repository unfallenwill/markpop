# markdown2svg

把 Markdown **直接**渲染成 SVG —— 不经过 HTML，不启动浏览器，不依赖任何字体文件。

## 为什么

传统方案 `markdown → HTML → 无头浏览器截图` 有三个问题：慢、依赖重、输出是位图。
这里用纯 Rust 重写整条管线：

1. **解析**：手写块级 / 行内解析器（CommonMark 子集 + GFM 表格、删除线）
2. **布局**：把 AST 变成带坐标的几何图元（段落、引用、列表、表格）
3. **序列化**：直接写 SVG 标签，缩放通过 `viewBox` 实现，矢量无损

> 相同输入永远产生相同字节 —— 度量用的是确定性字宽表，而不是系统字体。

## 支持的语法

| 特性 | 状态 | 说明 |
|---|---|---|
| 标题 | ✅ | ATX `#` 与 Setext `===` |
| 强调 | ✅ | `*斜体*`、`**粗体**`、`***两者***` |
| 删除线 | ✅ | GFM `~~text~~` |
| 行内代码 | ✅ | 带背景色块 |
| 代码块 | ✅ | 围栏式 + 缩进式，附语言标签 |
| 链接 | ✅ | 行内式、自动链接 `<https://…>` |
| 图片 | ✅ | 渲染为占位图框 + URL 标题 |
| 引用 | ✅ | 支持任意层嵌套 |
| 列表 | ✅ | 有序 / 无序、松散 / 紧凑、嵌套 |
| 表格 | ✅ | 支持 `:---` 对齐 |
| 分隔线 | ✅ | `---` / `***` / `___` |
| 分页 | ✅ | `--page-height`，按块切分 |
| 目录 | ✅ | `--toc`，按标题层级嵌套 |
| 输出格式 | ✅ | `--format svg/png/webp`，PNG/WebP 由内置光栅器生成 |
| 排版配置 | ✅ | 字号、字距、行距、字体栈、分栏，均可用参数或 API 配置 |
| 画布预设 | ✅ | `--canvas a4` 自动分页、`--canvas mobile` 移动端长图 |

### 中文与 CJK

中日韩字符按全角（1 em）度量，逐字断行；中英混排不会溢出画布：

```
中文测试内容中文测试内容中文测试内容中文测试内容
```

## 用法

```bash
# 标准输入 / 输出
cat README.md | markdown2svg > README.svg

# 指定文件、主题与缩放
markdown2svg doc.md --theme dark --zoom 150 -o doc.svg

# 自定义画布宽度
markdown2svg doc.md --width 1200 -o doc.svg

# 分页 + 目录（输出 page-1.svg、page-2.svg…）
markdown2svg doc.md --toc --page-height 1123 -o page.svg

# 位图输出：PNG / 无损 WebP（内置软件光栅化，不用浏览器）
markdown2svg doc.md --format png -o doc.png
markdown2svg doc.md --format webp -o doc.webp

# 排版配置：字号 / 字距 / 行距 / 字体 / 列数
markdown2svg doc.md --font-size 17 --line-height 1.75 --letter-spacing 0.01 \n  --font-family "Georgia, serif" --columns 2 -o doc.svg

# 画布预设：A4 自动分页 / 移动端长图
markdown2svg doc.md --canvas a4 -o page.svg
markdown2svg doc.md --canvas mobile -o long.svg
```

作为库使用：

```rust
let svg = markdown2svg::render("# Hello\n\nWorld");

let opts = markdown2svg::Options::default()
    .with_theme(markdown2svg::Theme::dark())
    .with_zoom(1.5);
let svg = markdown2svg::render_with("# Hi", &opts).unwrap();
```

## 架构

```
Markdown
  → parse     (ast::Document)   块级 / 行内
  → layout    (scene::Scene)    几何图元
  → svg       (String)          纯序列化
```

每一层都是纯函数，可以单独断言：

- 解析层断言 AST
- 布局层断言坐标与图元
- 序列化层断言标记

### 扩展点

- **主题**：`Theme` 是纯数据，从 `Theme::light()` 改字段即可
- **语法**：新增 `Block` / `Inline` 变体，解析与布局各加一个分支
- **输出格式**：`Scene` 只有几何，另写一个序列化器即可
- **字体度量**：`text::Metrics` 是唯一的宽度来源

## 限制

- 不下载远程图片（输出始终自包含）
- 引用式链接 `[a][b]` 与 HTML 内联暂不支持
- 嵌套强调按等长标记配对，极端的 `***a** b*` 会退化为字面量
- 单个超过一页的段落无法在不重排文本的前提下切分，会溢出（列表、引用、表格、代码块都会自动拆分）

---

*由 markdown2svg 自身渲染生成，100% 缩放。*
