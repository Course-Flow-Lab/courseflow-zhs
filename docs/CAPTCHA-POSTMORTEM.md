# 智慧树共享课验证码问题复盘与修复方案

> 状态：已定位并完成 0.3.10–0.3.13 的修复与部署；真实账号复测发现当前学习页给的是非拼图挑战，自动拼图仍无法完成该类验证。本文记录背景、证据、已做修改和后续可选方案。
>
> 安全说明：本文不记录 Cookie、站点令牌、验证码图片、账号名称或任务编号。验证码编号、`ev` 字段名和公开页面脚本可以记录。

## 1. 结论摘要

这次问题不是 ddddocr 识别失败，也不是验证码弹窗没有加载，而是四个独立问题叠加：

1. **易盾滑块存在约 10.5px 的启动死区。** 官方拼图会先吃掉一段指针位移才开始移动，模块按缺口距离原样拖动，拼图最终短约 10px，易盾返回 `300(unpass)`，`captcha.js` 进入 `failed`，任务被暂停。这是最初报错“官方验证码组件未通过验证”的直接原因。
2. **打开视频和保存进度的 `ev` 不是同一套字段。** 官方学习页在 `prelearningNoteApi` 后使用五字段 `evLog`；在 `saveLearningCInTime` 前使用九字段 `evLog`。模块最初对两者都使用九字段，后来又对两者都使用五字段，均会造成易盾令牌有效但 `validateSlideToken` 拒绝。
3. **验证码组件编号错误。** 模块长期使用 `75f9f716460a422f89a628f50fd8cc2b`，而当前学习页 `ableCaptchaCommon.js` 实际使用 `4da3050565514a35a50541b0e1f54538`。前者能加载出拼图并返回令牌，但属于另一个验证码场景，平台不认可。
4. **当前学习页的正确组件给的是非拼图挑战。** 改用 `4da305...` 后，真实账号复测得到的是“图形点选或其他验证”，不是拼图。ZHS 模块目前只支持易盾拼图；遇到点选类挑战会按设计暂停并提示人工处理。

因此，0.3.13 解决了“用错组件、错误地报组件未通过”的问题，但没有、也不应该声称能自动完成当前的非拼图挑战。

## 2. 背景

### 2.1 功能目标

智慧树共享课学习在开始视频或保存进度时会触发官方验证码。Framework 0.1.13 起提供可选的本地识别服务：

- 后端：`ddddocr` 1.5.6，只接收图片，不接收平台 Cookie、密码或令牌；
- 模块：ZHS 通过 `cw.captcha.slide_match` 识别拼图缺口，通过 `cw.browser.*` 读取官方组件图片并拖动；
- 确认：识别坐标不代表验证通过，必须由智慧树 `validateSlideToken` 返回 `status=200` 且 `pass=true`；
- 计时：验证及暂停时间不计入学习时长，失败时保留检查点并暂停。

设计目标是覆盖共享课学习、短时试跑和每日复习中的**拼图**验证。人脸、短信、图形点选、登录、作业考试等仍由用户处理。

### 2.2 原链路

```text
学习步骤
  ├─ prelearningNote / saveDatabaseIntervalTimeV2
  │      └─ 平台返回 code=-12（需要验证）
  ├─ captcha::start
  │      └─ cw.browser.open(purpose=captcha)
  │             └─ 官方源下的空文档 + 注入平台 Cookie
  ├─ 页面脚本加载易盾组件
  │      ├─ 拼图 → 读取背景/拼图原图
  │      │      ├─ cw.captcha.slide_match
  │      │      ├─ cw.browser.mouse(drag)
  │      │      └─ 等待 onVerify 令牌
  │      └─ 非拼图 → unsupported
  └─ validateSlideToken(token, ev, checkType=1)
         ├─ 通过 → 恢复学习
         └─ 失败 → needs_human（保留检查点、关闭页面）
```

原始报错的映射关系：

```text
易盾 onVerify 返回 error（300 unpass）
  → captcha.js state.failed = true
  → captcha.rs 的 failed 分支
  → “官方验证码组件未通过验证，请在官方页面完成”
  → engine.ts “需要人工处理：自动滑块验证未完成：……”
```

### 2.3 环境

- 部署：Docker Compose，API / Web / captcha 三容器；
- Framework：0.1.17；
- ZHS：初始 0.3.9，最终 0.3.13；
- 验证码后端：`ddddocr` 1.5.6，`/health` 正常；
- 浏览器：镜像内 Chromium，headless，DPR=1，弹窗宽度固定 320px。

## 3. 现象与复现

### 3.1 原始现象

任务在共享课某个视频的保存阶段暂停，详情为：

```text
需要人工处理：自动滑块验证未完成：官方验证码组件未通过验证，请在官方页面完成
```

检查点只剩 `phase=captcha_retry`，没有保留页面、令牌或 `ev`。恢复任务后会再次尝试，仍然失败。

### 3.2 OCR 后端检查

在部署实例上直接调用 `/slide_match`：

- 背景图 320×160，拼图自然尺寸约 60×158；
- 返回示例：`{"target":[122,90,179,138],"target_x":2,"target_y":89}`；
- 目标缺口在背景图中的位置与返回坐标一致；
- 图片读取、原图 Base64、透明通道裁剪都正常。

结论：**OCR/模板匹配没有问题**，识别出的缺口坐标是可用的。

### 3.3 真实官方组件复现

不上真实账号，使用官方源下的空文档加载易盾组件，按生产同样的 12 步 CDP 拖动：

| 拖动距离 | 易盾 `/api/v3/check` | `onVerify` |
| --- | --- | --- |
| 恰好等于识别缺口（如 237px） | `result:false` | `300(unpass)` |
| 缺口 + 10px | `result:true` | 返回 `validate` 令牌 |
| 缺口 + 10.5px | `result:true` | 返回 `validate` 令牌 |
| 缺口 + 11px | `result:true` | 返回 `validate` 令牌 |

同时读取 DOM：

```text
拼图 style.left = 手柄位移 - 10.5px
```

在 20–240px 的拖动范围内基本恒定。也就是说，拖动手柄 237px，拼图只移动约 226.5px，最终比缺口少约 10.5px。

这是第一个确定根因：**滑块启动死区**。

## 4. 根因分析

### 4.1 滑块启动死区

易盾组件在拖动开始时先消耗约 10.5px 的指针位移，拼图才开始移动。模块原来的算法是：

```text
distance = gap * 显示宽度 / 原图宽度 - 拼图初始 left
```

然后直接把 `distance` 作为手柄位移。这个距离对“拼图应该移动多少”是正确的，但对“手柄应该移动多少”少了死区补偿。

修复：在手柄位移上增加 `SLIDER_DEAD_ZONE = 10.5`。该值在 320px 弹窗、DPR=1 下校准；如果弹窗宽度改变，需要重新测量。

### 4.2 验证码组件编号

官方学习页当前使用的组件编号：

```text
ableCaptchaCommon.js → 4da3050565514a35a50541b0e1f54538
```

模块原来使用：

```text
75f9f716460a422f89a628f50fd8cc2b
```

`75f9...` 能加载出拼图并返回有效的易盾令牌，但它属于另一个验证码场景。`validateSlideToken` 校验的是学习页组件场景，因此返回 `pass:false`。这解释了为什么“易盾已经通过，平台仍说未通过”。

修复：把 `captcha.js` 的 `captchaId` 改为 `4da305...`，并增加回归断言。

### 4.3 `ev` 上下文

官方学习页的 `evLog` 由 `D26666.Z([...])` 生成，和保存请求的 `sdsew` 一样使用 `zzpttjd` 掩码。但打开和保存的字段不同。

#### 打开视频（`prelearningNoteApi` 之后）

```text
[recruitId, lessonId, lessonVideoId, videoId, chapterId]
```

五字段。

#### 保存进度（`saveLearningCInTime` 之前）

```text
[recruitId, chapterId, courseId, lessonId, position,
 totalStudyTime, lastViewVideoId, smallLessonId, playTimes]
```

九字段，其中 `position` 使用零填充的 `HH:MM:SS`。

模块最初对打开和保存都用九字段；0.3.11 又改成两者都用五字段；0.3.12 才拆成两个构造函数。仅修正 `ev` 仍不能解决组件编号错误，所以真实账号测试直到 0.3.13 才暴露出“非拼图”这一层。

需要说明：模块实际保存接口是旧的 `saveDatabaseIntervalTimeV2`，上面的九字段来自当前学习页的 `saveLearningCInTime`；两者共用 `validateSlideToken` 的概率很高，但在正确组件的拼图挑战上尚未完成真实账号闭环，仍需验证。

### 4.4 非拼图挑战

0.3.13 使用正确的 `4da305...` 后，真实账号复测得到：

```text
需要人工处理：自动滑块验证未完成：当前为图形点选或其他验证，请在官方页面完成
```

`captcha.js` 的判断：

```js
if (popup.classList.contains('yidun--space')) return { state: 'unsupported' };
if (!popup.classList.contains('yidun--jigsaw')) return { state: 'unsupported' };
```

这说明当前弹窗不是 `yidun--jigsaw`。可能是 `yidun--space`（图形点选），也可能是其他类型。模块只实现了拼图，因此按设计暂停。

这里有两个尚未完全确认的问题：

1. 正确组件在学习页的真实上下文中是否**总是**给点选，还是空文档/headless/风险评分导致它走了备用点选组件？
2. 如果真实页面会给拼图，模块是否应该加载更接近官方的页面，而不是空文档？

这两个问题需要下一步实验，不能靠猜。

## 5. 已实施修复

### 5.1 提交与版本

| 提交 | 版本 | 修复 |
| --- | --- | --- |
| `5bbe4e8` | 0.3.10 | 滑块启动死区补偿 10.5px |
| `a34c264` | 0.3.11 | 打开视频使用官方五字段 `ev` |
| `d1034ae` | 0.3.12 | 保存进度使用官方九字段 `ev` 和零填充时间 |
| `795ed6b` | 0.3.13 | 使用学习页正确的 `captchaId`，增加回归断言 |

每版都按模块升级流程部署：

- 备份数据库、主密钥、模块目录、Compose 配置；
- 通过管理员模块安装接口安装，不停止整个 API；
- 旧模块进程被替换，新模块加载并 health check；
- 暂停任务显式迁移到新模块版本，检查点保留；
- 失败时回退到上一模块版本，数据库快照保留。

### 5.2 代码要点

`courseflow-zhs/src/zhs/captcha.rs`：

```rust
/// 易盾滑块在组件开始移动拼图前会先吃掉约 10 CSS 像素的指针位移。
const SLIDER_DEAD_ZONE: f64 = 10.5;

let distance = gap * width / natural - piece_left + SLIDER_DEAD_ZONE;
```

`courseflow-zhs/src/zhs/captcha.js`：

```js
captchaId: '4da3050565514a35a50541b0e1f54538'
```

`courseflow-zhs/src/zhs/live.rs`：

```rust
fn captcha_ev_open(...) -> String {
    encode_study_fields(&[
        course.recruit_id.clone(),
        lesson.to_owned(),
        small.unwrap_or("0").to_owned(),
        media.to_owned(),
        chapter.to_owned(),
    ])
}

fn captcha_ev_save(..., position, total, played) -> String {
    encode_study_fields(&[
        course.recruit_id.clone(),
        chapter.to_owned(),
        course.id.clone(),
        lesson.to_owned(),
        position_text(position),
        total.to_string(),
        media.to_owned(),
        small.unwrap_or("0").to_owned(),
        played.to_string(),
    ])
}
```

`courseflow.module.json` 增加了逐版本兼容声明，例如：

```json
"checkpoint_compatible_from": ["0.3.8", "0.3.9", "0.3.10", "0.3.11", "0.3.12"]
```

### 5.3 部署后的状态

- ZHS 模块 0.3.13 已加载并 running；
- API、Web、captcha 容器健康；
- 超星任务没有被重启，仍在运行；
- 两个暂停的智慧树学习任务已迁移到 0.3.13；
- 其中一个复测时遇到非拼图挑战，暂停在“当前为图形点选或其他验证”；
- 另一个保持暂停，未自动继续。

## 6. 当前状态与限制

### 6.1 已经解决的

- 滑块启动死区导致的 `300(unpass)`；
- 打开/保存 `ev` 字段混用；
- 错误组件编号导致的“易盾通过但平台拒绝”；
- 错误信息不再把“组件编号不对”伪装成“组件未通过”。

### 6.2 尚未解决的

- 当前学习页正确组件给的是非拼图挑战，模块无法自动完成；
- 自动拼图只覆盖 `yidun--jigsaw`；
- 点选类挑战没有识别后端和拖动/点击实现；
- 空文档是否导致风险评分升高、是否应加载真实学习页，尚未做 A/B 验证。

## 7. 可能的修复方案

### 7.1 方案 A：明确人工兜底（短期推荐）

保持当前行为，但把“非拼图”处理得更明确：

1. 在 `captcha.js` 返回 `challenge_type`（`jigsaw` / `space` / `unknown`）和 `captcha_id`，不要只返回 `unsupported`；
2. 在任务详情显示：“当前为图形点选/其他验证，自动拼图不支持，请在官方页面完成后继续”；
3. 关闭验证码页面，保留检查点，不累计学习时长；
4. 文档明确：点选、人脸、短信、登录验证需要人工；
5. 不自动重试点选挑战，避免制造无意义的失败记录。

优点：成本低、行为诚实、不会用错误令牌欺骗平台。
缺点：用户仍需人工完成验证，任务不能全自动。

### 7.2 方案 B：找回“拼图主组件”或使用更真实的页面

需要验证以下假设：

> 当前 `4da305...` 在空文档/headless 下走了备用点选，而真实学习页会使用拼图主组件。

实验步骤：

1. 用同一账号、同一任务会话打开真实 `stuStudy` 页面，不自动播放，只观察验证码弹窗类型；
2. 分别测试：
   - 空文档 + 原始 `initNECaptcha`（当前实现）；
   - 空文档 + 官方 `ableCaptchaCommon.js` 包装；
   - 真实 `stuStudy` 页面 + 官方包装；
   - 真实页面但拦截视频播放器初始化。
3. 记录每次的 `captchaId`、弹窗 class、挑战类型、是否能拿到令牌、`validateSlideToken` 结果；
4. 如果真实页面稳定给拼图，考虑把 `cw.browser.open` 的验证码页面从“空文档”改为“官方页面 + 禁用播放”，或者只加载官方验证码包装脚本；
5. 如果真实页面仍然给点选，说明平台已经把学习验证码切换为点选，拼图方案本身过时。

风险：

- 加载真实页面可能启动视频、触发额外的平台心跳或风控；
- 页面脚本体积大、依赖多，可能影响 60 秒验证窗口；
- 需要框架提供更灵活的 `cw.browser.open` 选项（是否拦截文档、是否允许页面脚本）。

### 7.3 方案 C：增加图形点选识别

如果确认学习页确实使用 `yidun--space`（点选），需要新增一条完整链路：

1. 页面脚本识别 `yidun--space`，读取：
   - 背景图；
   - 题目/提示文字（例如“请依次点击……”）；
   - 画布尺寸和缩放比例；
2. 后端新增点选模型接口，例如 `POST /click`：
   - 输入背景图、提示文字、图片尺寸；
   - 输出按顺序的点击坐标 `[[x,y], ...]`；
   - 继续使用独立直连，不接收 Cookie/令牌。
3. 模块根据坐标和页面缩放计算屏幕坐标，按顺序派发真实鼠标点击；
4. 等待 `onVerify`，再用 `validateSlideToken` 确认；
5. 失败、超时或类型未知时保留检查点并暂停。

模型选择：

- 当前 ddddocr 1.5.6 **没有**点选接口，只有 `classification`、`detection`、`slide_match`、`slide_comparison`；
- 不能直接升级到 1.6.1：文档已记录其 `/slide_match` 关键字参数不兼容、返回格式变化；
- 需要额外模型（点选检测/文字定位/顺序模型）或单独服务；
- 需要真实点选样本、坐标标注、顺序标注和准确率验收；
- 需要处理点选顺序、重叠目标、背景缩放、移动端/桌面差异；
- 这是新功能，不是一次小修复，工作量明显大于本次拼图问题。

优点：可能恢复全自动。
缺点：模型/数据/维护成本高，且平台随时可能再换类型。

### 7.4 方案 D：诊断与可观测性

无论选 A/B/C，都建议补充：

1. 在模块日志中记录（debug 级别，不含令牌/图片）：
   - `captcha_id`
   - 弹窗 class / challenge type
   - `ev` 的字段数量或哈希，不记录明文上下文
   - `validateSlideToken` 的 `status`、`pass`、`code`、`msg`
2. 在任务事件中区分：
   - 组件未加载；
   - 拼图拖动被拒绝；
   - 令牌已拿到但平台确认失败；
   - 非拼图挑战。
3. 增加一个只读的“验证码诊断”入口，管理员可查看最近一次挑战类型和失败阶段；
4. 不要把 Cookie、令牌、图片写入日志、检查点或报告。

### 7.5 方案 E：测试与验收

自动测试：

- 死区补偿：不同 gap、不同缩放、不同拖动步数；
- `ev` 构造函数：打开五字段、保存九字段、零填充时间；
- `captchaId` 回归断言；
- 非拼图状态返回 `unsupported` 且不调用错误接口；
- 失败时关闭页面、保留检查点、不累计学习时长。

真实组件测试：

- 无账号官方组件：验证弹窗加载、图片读取、拼图拖动、`/check` 结果；
- 有账号：分别覆盖打开触发、保存触发、拼图、点选、平台拒绝、令牌过期；
- 记录通过率、平均耗时、失败阶段，而不是只看“任务是否暂停”。

验收标准：

- 拼图：`validateSlideToken` 返回 `status=200` 且 `pass=true`；
- 非拼图：明确暂停并提示人工，不伪造通过；
- 失败：不改变学习进度，不累计验证时间；
- 回退：模块和数据库备份可恢复，旧任务检查点不丢。

## 8. 建议顺序

1. **短期**：保留 0.3.13；把非拼图提示、诊断字段和文档补齐；用户遇到点选时人工完成。
2. **中期**：做 7.2 的真实页面/官方包装 A/B 实验，确认是否还能拿到拼图。如果能，优先改页面加载方式；成本低于训练点选模型。
3. **长期**：只有在确认平台稳定使用点选、且业务确实需要全自动时，才投入 7.3 的点选模型和完整链路。
4. 不建议回退到 `75f9...` 拼图：它对当前学习页的 `validateSlideToken` 无效，会回到“易盾通过但平台拒绝”的假成功。

## 9. 附录

### A. 关键测量

- 弹窗宽度：320px；
- 背景图：320×160；
- 拼图 natural：约 60×158，显示约 60.75×160；
- 死区：约 10.5px（CSS px，DPR=1）；
- 拖动 237px → 拼图约 226.5px；
- `gap + 10 / 10.5 / 11` 均通过 `/api/v3/check`；
- `gap` 原样拖动稳定失败。

### B. 官方字段

打开：

```text
[recruitId, lessonId, lessonVideoId, videoId, chapterId]
```

保存：

```text
[recruitId, chapterId, courseId, lessonId, position,
 totalStudyTime, lastViewVideoId, smallLessonId, playTimes]
```

### C. 错误映射

| 阶段 | 现象 | 模块结果 |
| --- | --- | --- |
| 组件加载失败 | 脚本/网络错误 | `failed` → 人工 |
| 拼图拖动被拒 | `/check result:false`、300 | `failed` → 人工 |
| 拿到令牌但平台拒绝 | `validateSlideToken pass:false` | 人工 |
| 非拼图挑战 | `yidun--space`/其他 | `unsupported` → 人工 |
| 平台确认通过 | `status=200, pass=true` | 恢复学习 |

### D. 回退思路

- 模块版本保留：安装目录同时保留 0.3.9–0.3.13；
- 数据库与主密钥在每次部署前有快照；
- 回退时先停止 API，恢复配对快照，再安装对应模块版本；
- 已迁移到 0.3.13 的任务不能直接回退到不声明兼容的旧模块，需要先恢复任务输入或使用支持 0.3.13 检查点的版本。
