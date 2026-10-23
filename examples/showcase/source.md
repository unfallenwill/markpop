# MarkPop

**MarkPop** 把 Markdown **直接**渲染成 SVG —— 不经过 HTML，不启动浏览器，不依赖任何字体文件。

## 为什么

传统方案 `markdown → HTML → 无头浏览器截图` 有三个问题：慢、依赖重、输出是位图。
这里用纯 Rust 重写整条管线：

1. **解析**：手写块级 / 行内解析器（CommonMark 0.31.2 全量 + GFM 扩展）
2. **布局**：把 AST 变成带坐标的几何图元（段落、引用、列表、表格）
3. **序列化**：直接写 SVG 标签，缩放通过 `viewBox` 实现，矢量无损

> 相同输入永远产生相同字节 —— 度量用的是确定性字宽表，而不是系统字体。

## 支持的语法

| 特性 | 状态 | 说明 |
|---|---|---|
| 标题 | ✅ | ATX `#` 与 Setext `===` |
| 强调 | ✅ | `*斜体*`、`**粗体**`、`***两者***`（完整分隔符算法） |
| 删除线 | ✅ | GFM `~~text~~` |
| 行内代码 | ✅ | 带背景色块 |
| 代码块 | ✅ | 围栏式 + 缩进式，附语言标签 |
| 链接 | ✅ | 行内式、引用式 `[text][label]`、自动链接 `<https://…>`、GFM 字面量 `www.` / 邮箱 |
| 引用定义 | ✅ | `[label]: url "title"`，可出现在文档任意位置 |
| HTML | ✅ | 块级 / 行内按语义渲染（表格、列表、引用、行内样式），不执行、无 CSS |
| 任务列表 | ✅ | GFM `- [ ]` / `- [x]`，绘制复选框 |
| 实体 | ✅ | 全部 HTML5 命名字符引用与数字引用 |
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

### 完整语法示例

- [x] 任务列表：GFM 复选框
- [ ] 待办事项
  - 嵌套列表支持有序与无序混排

> 引用里可以放代码块：
>
> ```rust
> println!("nested");
> ```

| 能力 | 说明 |
|:-----|:-----|
| 自动链接 | www.commonmark.org 与 someone@example.com |
| 引用式链接 | [CommonMark 规范][spec] |
| 实体 | &copy; &nbsp; ¥ &hellip; &#x1F600; |

[spec]: https://spec.commonmark.org "CommonMark 0.31.2"

### HTML 渲染

规范承认的 HTML 按语义渲染，标签本身不出现在输出里：

<table>
  <thead><tr><th align="left">写法</th><th align="left">渲染为</th></tr></thead>
  <tbody>
    <tr><td>&lt;table&gt; &lt;ul&gt; &lt;blockquote&gt; &lt;hr&gt;</td><td>真实表格 / 列表 / 引用 / 分隔线</td></tr>
    <tr><td>&lt;pre&gt; 与 &lt;h1&gt;…&lt;h6&gt;</td><td>代码块（含语言标签）与标题</td></tr>
    <tr><td>&lt;div&gt; 与未知标签</td><td>透明容器，内容正常排版</td></tr>
  </tbody>
</table>

上面这张表格就是 HTML —— 行内标签与 Markdown 混排：<b>加粗</b>、<i>斜体</i>、<code>行内代码</code>、<s>删除线</s>、<a href="https://spec.commonmark.org">链接</a>。

## 用法

```bash
# 标准输入 / 输出
cat README.md | markpop > README.svg

# 指定文件、主题与缩放
markpop doc.md --theme dark --zoom 150 -o doc.svg

# 自定义画布宽度
markpop doc.md --width 1200 -o doc.svg

# 分页 + 目录（输出 page-1.svg、page-2.svg…）
markpop doc.md --toc --page-height 1123 -o page.svg

# 位图输出：PNG / 无损 WebP（内置软件光栅化，不用浏览器）
markpop doc.md --format png -o doc.png
markpop doc.md --format webp -o doc.webp

# 排版配置：字号 / 字距 / 行距 / 字体 / 列数
markpop doc.md --font-size 17 --line-height 1.75 --letter-spacing 0.01 \n  --font-family "Georgia, serif" --columns 2 -o doc.svg

# 画布预设：A4 自动分页 / 移动端长图
markpop doc.md --canvas a4 -o page.svg
markpop doc.md --canvas mobile -o long.svg
```

作为库使用：

```rust
let svg = markpop::render("# Hello\n\nWorld");

let opts = markpop::Options::default()
    .with_theme(markpop::Theme::dark())
    .with_zoom(1.5);
let svg = markpop::render_with("# Hi", &opts).unwrap();
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

- 不下载远程图片，也不解析图片内容（输出始终自包含）
- HTML 按规范边界渲染：不执行脚本、无 CSS、不加载任何外部资源，未知标签为透明容器
- 超出画布宽度的单个长段落无法在不重排文本的前提下切分（列表、引用、表格、代码块都会自动拆分）

---

*由 markpop 自身渲染生成，100% 缩放。*
