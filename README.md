# Courseflow 智慧树模块

`courseflow-zhs` 是智慧树平台的独立 Rust 模块，通过 stdin／stdout 的 NDJSON JSON-RPC 接入 [Courseflow 框架](https://github.com/Course-Flow-Lab/courseflow-framework)。模块 API 为 1，当前模块版本为 0.3.2，需要框架 `>=0.1.3 <0.2.0`。

平台协议和官方页面脚本在本仓库；HTTP 凭据、Cookie 容器、持久化加密、模型密钥、浏览器进程和任务调度由框架管理。模块独立运行，不导入或启动其他平台工程。

## 模块定义

框架侧对模块清单、宿主服务和生命周期的通用说明见框架 README 的「模块系统」；这里只列智慧树特有的取值。

| 项 | 值 |
| --- | --- |
| 模块 id／平台 id | `zhs` |
| 名称 | 智慧树 |
| 版本／`moduleApi` | 0.3.2／1 |
| 框架范围 | `engines.framework`: `>=0.1.3 <0.2.0` |
| 运行形态 | `runtime.engine`: `binary`，入口 `target/release/courseflow-zhs` |
| 登录方式 | `qr`（扫码） |
| 声明操作 | `study`、`answer`、`review`、`meeting`、`mastery`、`qa`、`ask`、`trial` |
| 单元类型 | `video`、`document` |
| 最高倍速 | `max_speed`: 1.5 |
| 登录保活 | `keepalive_seconds`: 300，框架定期调用 `auth.keepalive` |
| 配置项 | `source`：`live`（默认，真实平台）或 `fixture`（离线记录） |
| 网络许可 | `zhihuishu.com`、`*.zhihuishu.com` |
| 其他权限 | `browser: true`（官方页面与播放器）、`ai: true`（模型生成由宿主执行） |
| 使用的宿主服务 | `cw.net.fetch`、`cw.state`、`cw.session.identity`、`cw.ai.generate`、`cw.browser.*`、`cw.peers.*`、`cw.clock.now`、`cw.log` |
| 清单文件 | [`courseflow.module.json`](courseflow.module.json) |

## 功能

| 类别 | 已实现的行为 |
| --- | --- |
| 扫码登录 | 真实二维码、状态轮询、Passport SSO、异常登录检查、身份验证、登录中心保活 |
| 共享课目录 | 分页课程、章节和小节、视频时长、已学进度、先修检查和日限额提示 |
| 共享课学习 | 默认原速、可选最高 1.5 倍速，先等待再保存；断点恢复、弹题、互动卡片暂停、平台完成回读 |
| 智慧课程 | 知识点及资源、视频计时和文档完成记录、平台回读 |
| 短时试跑 | 单个共享课视频、30–180 秒预算、保存后回读进度 |
| 每日复习 | 已完成共享课视频、按日期轮换、分钟预算、原速、保留原完成状态 |
| 作业与考试 | 共享课和智慧课程目录、开放状态与次数、题干和图片、已有答案保留、草稿、明确交卷、原作答回读 |
| 重做与只读查看 | 平台允许时显式重做；已提交作业和智慧课程试卷走只读路径，不为查看重新开卷 |
| 掌握度 | 知识图谱叶节点和随机节点、免考与满分跳过、无题跳过、最多三轮、成绩和正确答案回查 |
| 见面课 | 分页列表、官方回放、加载分钟观看历史后续播、原速播放、平台签到回读、初始化阶段诊断和播放三分钟无进展暂停 |
| 课程问答 | 官方页面动态签名、列表与筛选、模型或人工回答、每日限额、重复发布保护、已发布编号记录 |
| 课程提问 | 可编辑草稿、选择后发布、同课程成员显式开启的互助优先 |
| 成绩分析 | 计分规则、成绩、学习习惯分、课程互动分和问答统计 |
| 框架集成 | 人工补答、题图认证代理、个人／共享模型、答案记忆、Markdown／Word／JSON／CSV 导出 |

以上为实现范围，并不代表已在本次会话用真实账号逐项验收。逐项迁移与验收对照见[功能对照](docs/PARITY.md)。测试使用模拟宿主和显式 fixture，不会向真实平台保存学习进度、交卷、签到或发布内容。

当前平台题型适配单选（含类型 14）、多选和填空；宿主支持通用人工补答表单，但未知平台题型仍提示在官方页面完成。共享课已提交考试没有已确认的只读接口，使用原任务的本地报告导出，不重新开考。

## 构建与安装

需要 Rust ≥ 1.88，使用 stable 工具链：

```sh
cargo +stable test --locked
cargo +stable build --release --locked
```

清单入口为 `target/release/courseflow-zhs`。安装前先取得框架；已经有本地检出时跳过第一步：

```sh
# 1. 取得框架（已有检出可跳过 clone）
git clone https://github.com/Course-Flow-Lab/courseflow-framework.git ../courseflow-framework
(cd ../courseflow-framework && npm ci)

# 2. 在 courseflow-framework 中安装本模块
npm run cli -- module add ../courseflow-zhs
npm run cli -- module config zhs '{"source":"live"}'
```

服务与 CLI 的 `CF_DATA_DIR` 必须相同。源码、测试数据和平台私有资料不放在同一目录。

| 设置 | 默认值 | 含义 |
| --- | --- | --- |
| `CF_MODULE_SETTINGS` | `{}` | 宿主传入的模块配置 JSON |
| `source` | `live` | 真实平台；只有显式 `fixture` 才使用离线样例 |

无效 JSON、非对象或未知 source 会输出中文错误并退出，不静默切换到演示模式。`CF_MODULE_SETTINGS` 应由框架的模块设置提供。

离线验证使用独立数据目录，再显式设置：

```sh
npm run cli -- module config zhs '{"source":"fixture"}'
```

fixture 的二维码和成绩为样例。真实账号部署必须明确使用 `live`。

## 宿主方法

基础接口包括 `platform.describe`、`auth.begin/poll/complete/verify`、`catalog.courses/units` 和 `study.step`。功能扩展包括：

- `auth.keepalive`：只读刷新 Passport 登录页。
- `catalog.papers/mastery/meetings/score`：试卷、掌握度、见面课和成绩。
- `exam.open/review/redo/write/heartbeat/readback`：分段开卷、只读查看、显式重做、草稿、计时与结果回读。
- `qa.board/posts`：课程问题和当前账号发布记录。
- `activity.step`：见面课、回答和提问的分步执行。

所有外部访问通过 `cw.net.fetch`、`cw.state`、`cw.session.identity`、`cw.browser.*`、`cw.peers.*`、`cw.clock.now` 和 `cw.log`。模型生成由宿主编排，模块不接收模型密钥。完整 DTO 和调用约定见框架的[中文模块协议](https://github.com/Course-Flow-Lab/courseflow-framework/blob/main/spec/module-api-v1.md)。

公共 DTO 中不包含 `native`。平台原始目录、作答映射、开卷缓存、发布意图保存在宿主加密状态中，用户和模块之间隔离。stdout 只输出协议，日志写 stderr。

## 恢复行为

- 学习每段先等待，再保存。恢复时使用执行代次重新计时，不把暂停时间算作学习时间。
- 智慧课程长试卷每次读取少量题目；继续使用原开卷缓存，不重复 `openExam`。
- 草稿排除平台已有答案；人工修改需要宿主提供显式覆盖许可。
- 交卷前宿主保存操作锁。响应未知时只回读原作答，不重复交卷。
- 问答发布前保存意图；没有确认平台记录编号时停止，不自动重发。
- 见面课使用官方页面和播放器，保留其旧版 `Map` 行为；加载观看历史后寻找缺失分钟，不伪造签到。框架 0.1.2 保留父域登录 Cookie 的作用范围，模块给正常登录跳转保留最多三十秒；持续跳转或平台明确鉴权失败时暂停任务，重新连接后可核对并继续。初始化不依赖官方播放后会复位的 `videoFlag/watchHistoryFlag`，兼容旧版与原生 `Map` 的时间轴；初始化超过六十秒和播放连续三分钟无进展分别提示并保留重试检查点。诊断可在框架任务详情导出日志。
- 验证码、人脸、额外考试验证、未开放活动、无回放和未知控件保留人工处理状态。

模块是可信二进制，协议约束不能替代操作系统沙箱。框架的网络白名单约束宿主 HTTP 请求及浏览器入口，Chromium 仍自行加载官方页面内部资源。

## 目录

```text
src/
  main.rs、rpc.rs、host.rs   进程、协议与宿主调用
  platform.rs、error.rs      描述、方法分派和错误类型
  zhs/
    session.rs              扫码与 SSO
    live.rs、crypto.rs       共享课协议、动态网关密钥与签名
    catalog.rs、study.rs     公共课程及离线学习样例
    features/
      assessment.rs         共享课和智慧课程作业考试
      mastery.rs            掌握度图谱、练习和结果
      smart.rs              智慧课程资源
      questions.rs          题目、原答案及图片解析
      operations.rs         扩展接口、问答和见面课
      meeting.js            官方回放页面驱动
      types.rs              模块内部的平台私有记录
fixtures/                   无真实账号的离线目录样例
tests/contract.rs           模拟宿主与真实协议读写测试
```

## 验证范围

Cargo 测试包含清单一致性、扫码样例、公共字段、学习检查点、已完成跳过、签名向量、网关密钥、分段读取试卷、未知开卷防重发，以及见面课短暂登录跳转和持续登录过期的区分。框架另有 release 二进制的 fixture 集成测试，覆盖网页 API、人工补答、交卷确认和导出，并检查 Cookie 作用范围和官方页面脚本。

真实账号验收流程见[框架部署说明](https://github.com/Course-Flow-Lab/courseflow-framework/blob/main/docs/LOCAL-DEPLOYMENT.md)。学习写入、完整见面课签到、正式交卷和实际模型调用在本次实现中没有作为真实账号验收通过项。

## 许可证与来源

部分作业考试、掌握度、资源及页面协议使用 MIT 授权实现，版权与授权条件见 [LICENSE](LICENSE)。第三方许可证原文保留，以便正确保留版权及授权条件；项目说明文档使用简体中文。
