# `markpop::render`

把一份 Markdown 文档渲染成 SVG 字符串。

## 函数签名

```rust
pub fn render(markdown: &str) -> String
```

使用默认主题（`Theme::light()`）、100% 缩放渲染。

## 参数

| 参数 | 类型 | 说明 |
|---|---|---|
| `markdown` | `&str` | 文档源码。无法识别的输入会降级为文本，而不是报错。 |

## 返回值

SVG 文档字符串。标记是纯正的 SVG 1.1：只有 `<svg>`、`<rect>`、`<line>` 和
`<text>`，没有 CSS、没有滤镜、没有 `<foreignObject>`、没有任何外部引用。

## 示例

```rust
let svg = markpop::render("# 你好\n\n世界");
assert!(svg.starts_with("<svg"));
```

## 错误

该函数不会失败。解析是全函数：任何字节序列都能得到一个文档。

## 相关接口

- [`render_with`]：控制主题、缩放、画布和输出格式
- [`to_scene`]：只取几何信息，不做序列化

[`render_with`]: https://docs.rs/markpop
[`to_scene`]: https://docs.rs/markpop
