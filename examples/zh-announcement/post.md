# 发布文案

配套图片：`zh-announcement.webp`（750 × 2750，247 KB）。

> X 免费账号每条 280 字符，**CJK 算 2 个字符**，链接按 t.co 固定计 23。
> 下面的字数已按此规则核算过。

## 推荐版本（钩子型，201/280）

> 我们把文档构建从 4 分钟压到 400 毫秒。
>
> 做法是扔掉整条截图流水线：Markdown 直接写 SVG，PNG/WebP 用内置光栅器。零依赖，单二进制，输出逐字节可复现。
>
> 下面这张长图就是它自己渲染的 👇
>
> github.com/unfallenwill/markpop

## 备选

**痛点型（166/280）**——列三个具体代价

> 文档构建 4 分钟、构建镜像 300MB、产出一张不能搜索的 PNG。
>
> MarkPop：Markdown → SVG/PNG/WebP，零依赖，0.4 秒冷启动，文字还是文字。
>
> 长图在下面 👇
>
> github.com/unfallenwill/markpop

**极简型（151/280）**——适合已经有认知的受众

> Markdown → SVG / PNG / 无损 WebP。
>
> 不开浏览器、不截屏、零依赖。文字还是文字，能选、能搜、能无限缩放。
>
> 这条长图是它自己渲染的。
>
> github.com/unfallenwill/markpop

**English (270/280)**

> Render Markdown directly to SVG, PNG or lossless WebP.
>
> No browser, no screenshots, zero dependencies — one Rust binary.
>
> Text stays text: selectable, searchable, crisp at any zoom. Same input, same bytes.
>
> The long image below was rendered by it.
>
> github.com/unfallenwill/markpop

## 线程版（免费账号也能发，每条独立 ≤ 280）

**T1 — 钩子（201/280）**

> 我们把文档构建从 4 分钟压到 400 毫秒。
>
> 做法是扔掉整条截图流水线：Markdown 直接写 SVG，PNG/WebP 用内置光栅器。零依赖，单二进制，输出逐字节可复现。
>
> 下面这张长图就是它自己渲染的 👇

**T2 — 为什么（195/280）**

> 为什么要扔掉截图流水线？
>
> 因为位图是一句断言：文档到此为止。文字一旦压成像素，就不能搜索、不能重排、也不能改样式——换主题就得整站重渲染。
>
> MarkPop 直接写 SVG，文字还是文字。

**T3 — 实现细节（215/280）**

> 几个实现上的选择：
>
> · 版面度量用内置字宽表，不读系统字体 → 同输入永远同字节
> · 断行也在这个表上做 → 换机器页数和断行都不变
> · PNG/WebP 自己的编码器，不调系统库
> · 远程图片永不拉取，输出始终自包含

**T4 — 收尾（152/280）**

> CommonMark 0.31.2 全量 652 例 + GFM 扩展全过，429 个测试，零第三方依赖。
>
> cargo install --path .
>
> github.com/unfallenwill/markpop

## 配图顺序

X 一条最多 4 张，第一张是主视觉，后三张是缩略图：

| 顺序 | 文件 | 尺寸 | 作用 |
|---|---|---|---|
| 1 | `zh-announcement/zh-announcement.webp` | 750 × 2750 | 长图，承担全部说明 |
| 2 | `zh-magazine/zh-magazine.png` | 800 × 1288 | 双栏排版，视觉冲击最强 |
| 3 | `zh-dark-docs/zh-dark-docs.png` | 800 × 1096 | 深色主题 |
| 4 | `zh-print/zh-print.png` | 800 × 2074 | 打印稿，衬线 |

## 要点

- **长图是给人看懂的，推文是让人点开的。** 别把长图里的内容抄进推文。
- 第一行是钩子，3 秒内决定要不要展开图。
- 用具体数字（4 分钟 / 400 毫秒 / 300MB / 652 例），不用形容词。
- 链接放最后一行。
- 「这张长图就是它自己渲染的」是全篇最有效的一句——用产品介绍产品。
