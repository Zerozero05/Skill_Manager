# v0.2.7

Skill Manager 是面向 Windows 的本地与 GitHub Agent Skills 管理器：每个 Skill 保留一份运行文件，多个客户端通过目录链接共同使用。

## 本次变化

- 快捷键帮助使用独立浮层，在窗口边缘自动调整位置，支持键盘、缩放与完整内容滚动。
- 分类改为八色实心标签；夜间采用炭黑与石墨灰，明亮和护眼分别优化，并检查文字、控件和焦点对比度。
- 精简工作台、详情、导出、变化、客户端、设置及操作记录中的重复说明与空占位，保留真实来源、关键警告及全部原有操作。
- 修复目录跳转留白、任务徽标导致的内容区跳动，以及管理操作尚未保存完成时撤销按钮提前启用的问题。

这是首次公开发行。既有列表／卡片、分类收藏与撤销、GitHub 上传下载、必要／可选 ZIP 选择、默认／自定义位置、多标签、分屏、独立窗口和失败恢复继续适用。项目源码与 Skill 内容仓库分别维护。

## 下载选择

| 文件 | 用途 |
| --- | --- |
| `Skill_Manager_v0.2.7_windows_x64_portable.zip` | 解压后运行 `skill-manager.exe`；保留随包用户指南及第三方许可告知。 |
| `Skill_Manager_v0.2.7_windows_x64_setup.exe` | Windows x64 安装包。 |
| `Skill_Manager_v0.2.7_source.zip` | 本版本源码、锁文件、测试及构建配置，用于开发与维护。 |
| `SOURCE_MANIFEST.json`、`SHA256SUMS.txt` | 核对正式源码成员及各发行附件摘要。 |

[发行下载与附件清单](https://github.com/Zerozero05/Skill_Manager/releases/tag/v0.2.7) · [公开仓库](https://github.com/Zerozero05/Skill_Manager)

程序需要 Microsoft Edge WebView2 Runtime。当前交付为 Windows x64；其他系统、架构和其他电脑未完成验证。附件集合与摘要以发行页为准，不含个人设置、令牌、Skill 原件或本机维护记录。

## 兼容与数据保留

本版没有数据库格式迁移或依赖升级。已有 Skill 沿用原目录，管理数据与运行文件分别保存；更新前保留自己的管理数据和必要恢复材料。便携版默认在程序旁使用 `Skill_Manager_Data`，已有用户可通过 `--data-dir`、`SKILL_MANAGER_DATA_DIR` 或程序旁的 `data-directory.txt` 继续指向原管理数据目录，避免创建另一套管理记录。通用便携包不带个人数据目录侧文件。

必要文件默认加入 ZIP，可选包内与外部文件默认不选；GitHub 独有的已选资源可直接进入最终 ZIP，不在 Skill 目录生成副本。长期文件范围与本次导出选择仍分别处理。布局恢复只恢复导航，不自动重放上传、更新、删除或导出。

使用与恢复步骤见[用户指南](https://github.com/Zerozero05/Skill_Manager/blob/v0.2.7/docs/USER_GUIDE.md)。

## 验证与性能

本机构建与验证：**351 项 Rust 测试、136 项前端测试、93 组原生界面／流程验证**通过；TypeScript、Vite、格式、Clippy、Windows 优化版与 NSIS 安装包构建通过。原生流程区分真实业务、受控状态呈现、保存对话框返回值替代及真实回复延迟，没有把这些场景当作真实 GitHub 或模型调用结果。

云端 CI 以[Windows checks 的实际运行](https://github.com/Zerozero05/Skill_Manager/actions/workflows/check.yml)为准，正式发行页记录对应提交与运行结果；本机验证不等同于云端 CI。

同机 100 个合成 Skill、20 轮本地切换，保留 v0.2.6 与本版的完整实测增减：

| 指标 | v0.2.6 | v0.2.7 |
| --- | --- | --- |
| 首屏检测上界，含测试工具连接 | 708 ms | 612 ms |
| 说明切换中位耗时 | 16.82 ms | 16.91 ms |
| 说明切换 p95 | 36.47 ms | 37.63 ms |
| 文件切换中位耗时 | 12.95 ms | 13.36 ms |
| 文件切换 p95 | 48.25 ms | 61.98 ms |

说明与文件实际读取各 2 次，不随 20 轮切换增长。说明 p95 增加约 1.16 ms、文件 p95 增加约 13.73 ms，未声称所有指标改善。这是一次本地合成样本，不代表真实 GitHub、VPN、模型或其他电脑的加载速度。

未验证安装向导操作、系统保存对话框视觉、真实私有仓库写入、客户端模型实际调用、真实 GitHub／VPN 速度及其他电脑。

## 许可与来源

项目自身尚未指定开源许可证，公开源码不等于授予 MIT 等额外使用许可。第三方依赖继续遵循各自许可证，随程序保留 `THIRD_PARTY_NOTICES.txt`；方法参考与依赖说明见[来源记录](https://github.com/Zerozero05/Skill_Manager/blob/v0.2.7/docs/SOURCES.md)。本次未复制第三方应用源码或升级依赖。
