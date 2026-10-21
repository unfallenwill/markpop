# MarkPop

**把 Markdown 直接渲染成 SVG / PNG / 无损 WebP**

不开浏览器、不截屏、零第三方依赖。

```
markdown → parse → layout → SVG | PNG | WebP
```

纯 Rust 实现，429 个测试，编译产物单个二进制。

## 为什么要扔掉截图流水线

常规做法是 `Markdown → HTML → 无头浏览器 → 截图`。它能跑，但代价很实在：

| | 截图流水线 | MarkPop |
|---|---|---|
| 冷启动 | 约 90 秒 | 约 0.4 秒 |
| 依赖 | 浏览器 + 系统库 | 0 |
| 文字 | 压平成像素 | 仍然是文字 |
| 确定性 | 取决于装了哪些字体 | 逐字节一致 |
| 远程图片 | 会去拉 | 永不拉取 |

> 位图是一句断言：文档到此为止。文字一旦成了像素，就不能搜索、不能重排、也不能改样式。

## 三条命令

```bash
cargo install --path .

markpop doc.md -o doc.svg              # 矢量
markpop doc.md -f webp -o doc.webp      # 无损位图
markpop doc.md --canvas a4 -o page.svg  # A4 分页
```

## 它能做什么

- **矢量输出** —— SVG 里文字还是文字，任意缩放不糊
- **零依赖** —— 单二进制，无浏览器、无 JS 运行时、无 C 库
- **确定性** —— 同输入永远同字节，换台机器也不会错版
- **完整 Markdown** —— CommonMark 0.31.2 全量 652 例、GFM 扩展全过
- **中文友好** —— 全角度量、逐字断行，中英混排不溢出画布
- **自动分页** —— 代码块按行切、表格重复表头、标题不落单

## 库用法

```rust
let svg = markpop::render("# 你好\n\n世界");

let opts = markpop::Options::default()
    .with_theme(markpop::Theme::dark())
    .with_zoom(1.5);
```

## 顺手解决了一个麻烦

X 的长文（Articles）是 Premium 专属，免费账号只有 280 字符。
但**图片不受字符限制**——所以把内容渲染成长图，免费账号也能发。

这也是你正在看的这张图：它自己就是 MarkPop 渲染的。

```
markpop source.md --canvas mobile -f webp
```

## 上手

**github.com/unfallenwill/markpop**

```bash
git clone https://github.com/unfallenwill/markpop
cd markpop && cargo install --path .
```

零依赖、纯 Rust、单文件二进制，欢迎提 issue。
