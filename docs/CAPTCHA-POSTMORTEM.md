# 智慧树共享课验证码问题复盘与修复方案

> 状态：0.3.10–0.3.12 修复了滑块死区和 `ev` 拆分；0.3.13 曾把学习页组件误判为 `4da305...`，导致真实账号看到图形点选。0.3.14 依据官方包装器和 Playwright 全 no-op 路由更正为 `75f9...` 拼图组件，并已部署。真实账号复测已恢复拼图，但 `validateSlideToken` 仍未通过；已确认打开 `ev` 与官方五字段一致。0.3.15 改用学习页同款官方包装器，真实账号仍返回 `pass:false`；令牌来源不是唯一原因。0.3.16 增加 WebUI 人工验证：自动失败或平台拒绝后保留/重开官方页面，允许用户在网页中直接拖动/点击，而不再只看到暂停提示。
>
> 安全说明：本文不记录 Cookie、站点令牌、验证码图片、账号名称或任务编号。验证码编号、`ev` 字段名和公开页面脚本可以记录。

## 1. 结论摘要

这次问题由四个独立问题叠加：

1. **易盾滑块存在约 10.5px 的启动死区。** 官方拼图会先吃掉一段指针位移才开始移动，模块按缺口距离原样拖动，拼图最终短约 10px，易盾返回 `300(unpass)`，`captcha.js` 进入 `failed`。这是最初报错“官方验证码组件未通过验证”的直接原因。
2. **打开视频和保存进度的 `ev` 不是同一套字段。** 官方学习页在 `prelearningNoteApi` 后使用五字段 `evLog`；在 `saveLearningCInTime` 前使用九字段 `evLog`。模块最初对两者都使用九字段，后来又对两者都使用五字段，都会造成易盾令牌有效但 `validateSlideToken` 拒绝。
3. **验证码组件编号曾经被反向判断。** 学习页的 `ableCaptchaCommon` 以 `capType: "ImageFocusCombine"` 调用，选择 `75f9f716460a422f89a628f50fd8cc2b` 拼图组件；`4da3050565514a35a50541b0e1f54538` 是 `space` 回退，只有 `capType: "space"` 时才使用，显示图形点选。0.3.13 把两者弄反，真实账号因此从拼图变成了点选。
4. **真实账号的 `validateSlideToken` 仍有一次未通过。** 0.3.12 已经使用 `75f9...` 和九字段保存 `ev`，但真实账号复测仍返回“智慧树未确认滑块验证通过”。组件选择已查清；下一步需要抓取官方页面真实保存后的 `validateSlideToken` 请求，逐字段比较 `ev` 的值和格式，而不是继续猜组件。

因此：

- 0.3.13 不是修复，而是一次组件回归；
- 0.3.14 恢复 `75f9...` 拼图组件；
- Playwright 全 no-op 路由证明官方 HTML 与空文档都会得到同一映射；
- 0.3.14 是否能让真实账号通过，取决于剩余 `ev` 问题，不能仅凭组件恢复就宣布完成。

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
- ZHS：初始 0.3.9；0.3.10–0.3.14 已部署；
- 验证码后端：`ddddocr` 1.5.6，`/health` 正常；
- 浏览器：镜像内 Chromium，headless，DPR=1，弹窗宽度固定 320px；
- 实验工具：Playwright + 系统 Chromium，仅用于诊断，不进入运行时。

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

结论：**OCR／模板匹配没有问题**，识别出的缺口坐标是可用的。

### 3.3 死区复现

不上真实账号，使用官方源下的空文档加载易盾组件，按生产同样的 12 步 CDP 拖动：

| 拖动距离 | 易盾 `/api/v3/check` | `onVerify` |
| --- | --- | --- |
| 恰好等于识别缺口（如 237px） | `result:false` | `300(unpass)` |
| 缺口 + 10px | `result:true` | 返回 `validate` 令牌 |
| 缺口 + 10.5px | `result:true` | 返回 `validate` 令牌 |
| 缺口 + 11px | `result:true` | 返回 `validate` 令牌 |

DOM 实测：

```text
拼图 style.left = 手柄位移 - 10.5px
```

在 20–240px 的拖动范围内基本恒定。这是第一个确定根因：**滑块启动死区**。

### 3.4 官方组件选择复现

官方学习页的 `ableCaptchaCommon.js` 有两个编号：

```text
thqoE = 4da3050565514a35a50541b0e1f54538   // capType === 'space' 时使用
NKJcE = 75f9f716460a422f89a628f50fd8cc2b   // 其他 capType 时使用
```

学习页的 `capinitAbleYidunFallback` 调用参数：

```text
capType: 'ImageFocusCombine'
```

因此学习页应使用 `75f9...` 拼图，而不是 `4da305...` 点选。

用 Playwright 建立“全 no-op 路由”后，官方 HTML 与模块空文档的结果一致：

| 组件模式 | 请求编号 | 弹窗 |
| --- | --- | --- |
| 官方包装器 `ImageFocusCombine` | `75f9...` | `yidun--jigsaw` |
| 官方包装器 `space` | `4da305...` | `yidun--space` |
| 原始 `initNECaptcha` + `75f9...` | `75f9...` | `yidun--jigsaw` |
| 原始 `initNECaptcha` + `4da305...` | `4da305...` | `yidun--space` |

“全 no-op 路由”具体做法：

- 在官方 origin 下返回真实的 `stuStudy` HTML；
- 只允许 `ableCaptchaCommon.js`、`yidun-captcha.js`、`cstaticdun` 和易盾图片/API；
- 其余脚本、样式、XHR、fetch、图片和媒体一律返回空响应；
- 不注入账号 Cookie，不调用学习接口；
- 通过页面的官方包装器或原始加载器初始化验证码，读取弹窗 class 和实际请求的编号。

工具在 `tools/captcha-probe/`，详见该目录的 README。

## 4. 根因分析

### 4.1 滑块启动死区

易盾组件在拖动开始时先消耗约 10.5px 的指针位移，拼图才开始移动。模块原来的算法是：

```text
distance = gap * 显示宽度 / 原图宽度 - 拼图初始 left
```

然后把 `distance` 作为手柄位移。这个距离对“拼图应该移动多少”是正确的，但对“手柄应该移动多少”少了死区补偿。

修复：在手柄位移上增加 `SLIDER_DEAD_ZONE = 10.5`。该值在 320px 弹窗、DPR=1 下校准；如果弹窗宽度改变，需要重新测量。

### 4.2 组件映射与 0.3.13 回归

0.3.13 的提交说明写的是“使用学习页正确的组件编号”，但实际把两个编号弄反了：

- `75f9...`：拼图主组件，官方 `capType: "ImageFocusCombine"` 使用；
- `4da305...`：`space` 回退，官方 `capType: "space"` 使用。

0.3.13 改用 `4da305...` 后，真实账号从拼图变成了“请点击……图形”的点选挑战，于是 `captcha.js` 返回 `unsupported`。这不是平台改成点选，而是模块主动请求了备用点选组件。

修复：恢复 `75f9...`，并在注释和测试中固定“学习页使用拼图组件、`4da305` 是 `space` 回退”的事实。

### 4.3 `ev` 上下文

官方学习页的 `evLog` 由 `D26666.Z([...])` 生成，和保存请求的 `sdsew` 一样使用 `zzpttjd` 掩码。打开和保存的字段不同。

#### 打开视频（`prelearningNoteApi` 之后）

```text
[recruitId, lessonId, lessonVideoId, videoId, chapterId]
```

五字段。

#### 保存进度（`saveLearningCInTime` 之前）

从官方 `svcpEveryOnceInWhile` 提取的 `evLog` 数组：

```text
[recruitId, chapterId, courseId, lessonId, position,
 totalStudyTime, videoId, smallLessonId, elapsedSeconds]
```

九字段。最后一个字段是**本次保存的秒数**（`this[dec(0x2e9c)]`），不是 `playTimes` 播放次数。早期报告把该字段写成 `playTimes`，需要更正。

模块 0.3.12 的字段顺序与上述一致，但真实账号仍被 `validateSlideToken` 拒绝。剩余可疑点：

1. `position` 的格式：官方用 `Object(_0x117ec2['u'])(player.getPosition())`；模块 0.3.12 使用零填充 `HH:MM:SS`，需要确认官方helper是否返回 `H:MM:SS`；
2. `totalStudyTime` 是保存前还是保存后的累计值；
3. `courseId` 使用课程远程 ID 还是 `ccCourseId`；
4. `smallLessonId` 缺失时官方是 `0`、空串还是当前视频 ID；
5. 保存失败后恢复时，`position` 是否应回到失败前的值，而不是加过本次秒数。

在抓到官方一次真实保存后的 `validateSlideToken` 请求之前，不能确定是哪一项。仅恢复组件编号不会自动解决这个问题。

### 4.4 为什么 0.3.12 仍然失败

0.3.12 已经使用 `75f9...`，所以不是组件编号问题；它失败在 `validateSlideToken`，说明易盾令牌本身有效，但平台绑定上下文不匹配。

0.3.14 的临时调试进一步确认：本次是**打开下一段视频**触发的验证，`ev` 解码为：

```text
[463667, 1000337358, 1000244484, 67722997, 1000116295]
```

正好对应官方的 `[recruitId, lessonId, lessonVideoId, videoId, chapterId]` 五字段。平台返回 `status:"200"`、`pass:false`、`msg:"请求成功"`。因此剩余问题不在字段顺序或缺失，而更可能在：

- 易盾令牌与 `-12` 响应对应的挑战/会话不匹配；
- 平台侧对 headless/自动化环境的风险判定；
- `checkType`、请求签名或网关上下文与官方页面不同；
- 官方页面用的是 `initNECaptchaWithFallback`，而模块仍是原始 `initNECaptcha`。

下一步应优先对照官方页面的真实 `validateSlideToken` 请求和令牌来源，而不是继续改 `ev` 字段。

## 5. 已实施修复

### 5.1 提交与版本

| 提交 | 版本 | 修复 |
| --- | --- | --- |
| `5bbe4e8` | 0.3.10 | 滑块启动死区补偿 10.5px |
| `a34c264` | 0.3.11 | 打开视频使用官方五字段 `ev` |
| `d1034ae` | 0.3.12 | 保存进度使用官方九字段 `ev`（时间格式待真实请求复核） |
| `795ed6b` | 0.3.13 | **回归**：误用 `4da305...` 点选回退 |
| `09184a9` | 0.3.14 | 恢复 `75f9...` 拼图组件，增加 Playwright 探针和回归断言 |
| `4bf87f1` | 0.3.15 | 改用官方 `initAbleYidunFallback` 包装器，验证令牌来源 |
| `f36e3e5` | 0.3.16 | 自动失败后保留页面，WebUI 人工拖动/点击；300 秒超时 |
| `f6bb3bb` / `bb68162` | 0.3.16 | 平台拒绝后重开干净页面，并在新页面加载官方组件 |

0.3.10–0.3.15 已按模块升级流程部署；0.3.16 已与 Framework 0.1.18、WebUI 0.1.5 一起部署。人工验证页面已实际返回 320×215 的官方拼图截图。

### 5.2 代码要点

`courseflow-zhs/src/zhs/captcha.rs`：

```rust
/// 易盾滑块在组件开始移动拼图前会先吃掉约 10 CSS 像素的指针位移。
const SLIDER_DEAD_ZONE: f64 = 10.5;

let distance = gap * width / natural - piece_left + SLIDER_DEAD_ZONE;
```

`courseflow-zhs/src/zhs/captcha.js`：

```js
// 官方学习页 capType='ImageFocusCombine' 选择拼图组件。
// 4da305... 是 capType='space' 的点选回退。
captchaId: '75f9f716460a422f89a628f50fd8cc2b'
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

`courseflow.module.json` 增加了逐版本兼容声明：

```json
"checkpoint_compatible_from": ["0.3.8", "0.3.9", "0.3.10", "0.3.11", "0.3.12", "0.3.13"]
```

### 5.3 部署后的状态

- 远程当前为 ZHS 0.3.16、Framework 0.1.18、WebUI 0.1.5；三个容器健康；
- 模块 0.3.16 安装后，自动失败/平台拒绝会把任务保持在运行状态并显示“等待人工完成验证码”；
- 任务响应 `manual_captcha:true`，`GET /api/tasks/:id/captcha` 已返回 320×215 的官方拼图截图；
- 任务正在等待用户在 WebUI 中完成验证码；人工页面不会绕过平台确认，完成后仍需 `validateSlideToken` 通过；
- 超时或用户关闭后仍会保留检查点并暂停；
- 这次失败发生在打开下一段视频时。通过临时调试日志抓到了本次打开 `ev`：
  ```text
  [463667, 1000337358, 1000244484, 67722997, 1000116295]
  ```
  它与官方五字段打开 `ev` 的结构和值一致；
- 平台返回 `status:"200"`、`pass:false`、`msg:"请求成功"`，说明请求已被平台解析，失败发生在令牌/绑定校验，而不是 `ev` 字段缺失。

## 6. 当前状态与限制

### 6.1 已经解决的

- 滑块启动死区导致的 `300(unpass)`；
- 打开/保存 `ev` 字段混用；
- 组件编号映射：`75f9` 是拼图主组件，`4da305` 是 `space` 回退；
- 用 Playwright 全 no-op 路由复现官方 HTML 与空文档的一致结果。

### 6.2 尚未解决的

- 0.3.14 已恢复拼图，但真实账号的 `validateSlideToken` 仍未通过；
- 这次是打开下一段视频触发的验证，使用五字段打开 `ev`；调试确认字段和值与官方一致；
- 0.3.15 的官方包装器也返回 `status:200, pass:false`，所以原始/包装器令牌来源不是唯一原因；
- 还需要判断是平台挑战绑定、`checkType`/签名，还是平台对自动化环境/账号的风险判定；
- 0.3.16 已加入 WebUI 人工验证，让用户可以直接操作官方页面；平台确认仍不可绕过。任务页面在平台拒绝旧令牌后会重新打开干净验证码。

## 7. 后续修复方案

### 7.1 方案 A：抓官方请求并对照 `ev`（推荐）

1. ~~部署 0.3.14，把暂停任务迁移到 0.3.14~~（已完成）；
2. ~~继续任务，观察易盾是否显示拼图、是否返回令牌~~（已确认拼图恢复并拿到令牌）；
3. `validateSlideToken` 仍拒绝；已抓到本次打开 `ev`，字段和值与官方五字段完全一致；
4. 0.3.15 的官方包装器也失败，说明不是原始/包装器令牌来源差异；
5. 平台返回 `status:200, pass:false, msg:请求成功`，下一步不再猜 `ev`，而是：
   - 在官方页面完成一次验证，检查手动令牌是否能通过；
   - 对照官方 `validateSlideToken` 的 `checkType`、签名/网关上下文；
   - 检查平台账号/IP 是否被标记，或 `-12` 响应是否还有未被模块使用的挑战参数。

### 7.2 方案 B：继续完善 no-op 路由工具

当前工具已经可以：

- 返回官方 HTML；
- 加载官方 `ableCaptchaCommon`／`yidun-captcha`；
- 只放行易盾资源；
- 选择 `ImageFocusCombine` 或 `space`；
- 输出实际请求编号和弹窗 class。

后续可增加：

- `--capture-validate`：接入真实会话时只观察官方 `validateSlideToken` 请求，不提交；
- `--ev`：把候选 `ev` 传给工具做对照；
- 对官方 HTML 的资源白名单做快照测试；
- 在 CI 之外保留为手动诊断工具，不把真实平台脚本提交进仓库。

注意：工具不能代替真实账号验收，也不能把默认 fallback 令牌当成通过。

### 7.3 方案 C：图形点选支持

如果未来学习页真的改为 `capType: "space"`：

- 当前 ddddocr 1.5.6 没有点选接口，只有 `classification`、`detection`、`slide_match`、`slide_comparison`；
- 不能直接升级到 1.6.1：其 `/slide_match` 关键字参数不兼容、返回格式变化；
- 需要额外的点选模型、坐标/顺序样本和准确率验收；
- 页面需要读取提示文字、背景图、画布缩放，按顺序点击，再走 `validateSlideToken`。

这是新功能，不是当前修复的必要项。

### 7.4 方案 D：WebUI 人工验证码

0.3.16 与 Framework 0.1.18 增加：

- `cw.browser.open` 对验证码页面返回 `manual:true`；
- 模块自动失败/非拼图后进入 `manual` 步骤，保留页面最多 300 秒，任务详情显示“等待人工完成验证码”；
- 任务响应增加 `manual_captcha`；
- `GET /api/tasks/:id/captcha` 返回官方弹窗 PNG、页面坐标和尺寸；
- `POST /api/tasks/:id/captcha` 转发 `click`/`drag`，仅任务所有者可调用并需要 CSRF；
- WebUI 在任务卡片/详情显示“完成验证码”，弹窗中用 canvas 显示官方画面；
- 拖动时发送 `press`/`move`/`release`，并以约 120ms 刷新截图，拼图块会跟随指针实时移动；
- 站点管理提供无账号的“验证码拖动测试”入口，便于直接验证拖动链路；
- 用户操作后，模块读取官方 `validate` 令牌并继续 `validateSlideToken`。

这不是绕过平台校验：即使人工操作，平台仍必须确认通过。它把“必须到官方页面”改成“在 Courseflow 页面完成同一验证码”。

### 7.5 方案 E：人工兜底与诊断

无论组件是否恢复，都应保留：

- 非拼图挑战明确提示“当前为图形点选或其他验证，请在官方页面完成”；
- 失败时关闭页面、保留检查点、不累计学习时长；
- 日志记录 `captcha_id`、弹窗 class、`ev` 字段数量/哈希、`validateSlideToken` 的 `status/pass/code/msg`，不记录令牌、Cookie 或图片；
- 任务详情区分“组件未加载”“拼图被拒绝”“令牌已拿到但平台确认失败”“非拼图挑战”。

## 8. 建议顺序

1. ~~先纠正组件回归：部署 0.3.14，恢复 `75f9...` 拼图~~（已完成）。
2. ~~再对照令牌来源：部署 0.3.15 官方包装器~~（已完成，仍 `pass:false`）。
3. ~~部署 0.3.16 + Framework 0.1.18 + WebUI 0.1.5~~（已完成；人工截图已验证）。
4. **让用户在 WebUI 完成当前验证码**：如果平台确认仍失败，抓取该次 `validateSlideToken` 的上下文。
5. **手动令牌对照**：如果人工页面能通过而自动令牌不能，检查平台风险/挑战绑定；如果人工也不能，检查账号/IP 或平台限制。
6. **Playwright 的定位**：保留为 `tools/captcha-probe/` 的手动诊断工具；生产继续使用原生 CDP，暂不为一个探针替换整个浏览器层。
7. **点选支持**：等平台真的切换后再做，不在当前阶段投入模型和样本。

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
 totalStudyTime, videoId, smallLessonId, elapsedSeconds]
```

最后一个字段是本次保存秒数，不是播放次数。

### C. 组件映射

| 官方 `capType` | 编号 | 挑战 |
| --- | --- | --- |
| `ImageFocusCombine`（学习页） | `75f9...` | `yidun--jigsaw` |
| `space`（回退） | `4da305...` | `yidun--space` |

### D. 错误映射

| 阶段 | 现象 | 模块结果 |
| --- | --- | --- |
| 组件加载失败 | 脚本/网络错误 | `failed` → 人工 |
| 拼图拖动被拒 | `/check result:false`、300 | `failed` → 人工 |
| 拿到令牌但平台拒绝 | `validateSlideToken pass:false` | 人工 |
| 非拼图挑战 | `yidun--space`/其他 | `unsupported` → 人工 |
| 平台确认通过 | `status=200, pass=true` | 恢复学习 |

### E. 回退思路

- 模块版本保留：安装目录同时保留 0.3.9–0.3.14；
- 数据库与主密钥在每次部署前有快照；
- 回退时先停止 API，恢复配对快照，再安装对应模块版本；
- 已迁移到 0.3.14 的任务不能直接回退到不声明兼容的旧模块，需要先恢复任务输入或使用支持 0.3.14 检查点的版本。部署 0.3.14 前的快照可用于回退到 0.3.13。
