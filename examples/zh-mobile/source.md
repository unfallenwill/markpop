# 速查卡：把 README 做成图片

**目标**：一张可以在社交平台和聊天里直接转发的长图

README 截图通常有 800 px 宽，在手机上根本看不清。这张速查卡说明怎么用手机宽度
渲染同一份文档，并放大正文。

```bash
markpop README.md --canvas mobile -o share.svg
```

## 会得到什么

- 宽度 750 px 的连续长图，不分页
- 正文字号 24 px，而不是默认的 16 px
- 内边距 32 px，而不是默认的 48 px

## 调参

| 目的 | 参数 | 取值 |
|---|---|---|
| 更宽的画布 | `--width` | `800` |
| 更大的字号 | `--font-size` | `26` |
| 更松的行距 | `--line-height` | `1.7` |
| 深色背景 | `--theme` | `dark` |
| 更高分辨率 | `--zoom` | `150` |

## 配合位图格式

如果目标平台不接受 SVG，改成无损 WebP：

```bash
markpop README.md --canvas mobile --zoom 150 -f webp -o share.webp
```

## 清单

- [x] 用手机宽度渲染
- [x] 正文放大到 24 px
- [x] 导出无损 WebP 供聊天软件使用
- [ ] 在页眉加上项目 Logo
