Skill Manager 是面向 Windows 的本地与 GitHub Agent Skills 管理器：每个 Skill 保留一份运行文件，客户端通过目录链接共同使用。本版整合 v0.2.8、v0.2.9 候选中的修复与自动发现，并完成本地目录列表优化。

## 本次变化

1. 本地目录支持多选预览、原位登记及自动导入；新建 Skill 自动发现，忽略项可批量恢复。目录可改名、暂停、恢复和移除，原文件与已有连接保留。
2. 全窗口统一自动／手动检查。本地事件防抖与远端提交探测分别工作；手动模式仅在点击时检查。外部方式更新 GitHub 后，可靠来源自动关联，歧义保留待选择，不伪造历史安装基准。
3. 修复连续中文路径与有效绑定仍误报缺失的问题，文件检查和 ZIP 使用同一有效来源。延续导出清单监听恢复、跨窗口选择、原生关闭及客户端紧凑布局修复。
4. 忽略的 Skill 改为逐行名称／路径列表；自动导入目录紧凑对齐，“检查”保持可见，其余操作收进更多菜单。窄分栏自然换行，保留大字号、缩放、快捷键与批量操作。

## 下载选择

| 附件 | 用途 |
| --- | --- |
| `Skill_Manager_v0.2.10_windows_x64_portable.zip` | 解压后运行 `skill-manager.exe`；同包保留指南、第三方许可和路径侧文件。 |
| `Skill_Manager_v0.2.10_windows_x64_setup.exe` | Windows x64 NSIS 安装器；包内容已核验，安装向导尚未实测。 |
| `Skill_Manager_v0.2.10_source.zip` | 完整本版源码、锁文件、测试、说明与构建配置。 |
| `SOURCE_MANIFEST.json` | 源码成员摘要、源码提交、候选来源与安装器内部程序核验信息。 |
| `SHA256SUMS.txt` | 核对其余六项附件的 SHA256。 |
| `USER_GUIDE.md` | 使用与故障处理指南。 |
| `RELEASE_NOTES.md` | 本发行说明。 |

[发行页与附件](https://github.com/Zerozero05/Skill_Manager/releases/tag/v0.2.10) · [公开仓库](https://github.com/Zerozero05/Skill_Manager)

便携包与安装器使用同一份已验 EXE，程序 SHA256 为 `b8449fd56195ff0005e271f52fbf4bed096d40d4788228a5b0f3f0728b2771ad`。程序需要现有 Microsoft Edge WebView2 Runtime；附件不包含个人数据库、令牌、私人 Skill 或本机维护资料。

## 升级与兼容

- 正常退出旧版后替换程序文件，保留原管理数据、目录设置和 Skill 原位置。程序不自动移动 Skill，也不复制成多份客户端源码。
- 当前便携包附带 `data-directory.txt`，内容为 `.`，新使用时将数据放在程序同目录。既有用户继续沿用原侧文件、`--data-dir` 或环境变量指定的位置，避免创建另一套空记录。
- 没有数据库格式迁移。新增检查与目录字段提供兼容默认值；分类、收藏、来源、运行范围和客户端连接保持。相比 v0.2.7 包含 notify 8.2.1 本地目录监听依赖及对应许可。
- 下载、更新和提交继续预览并手动应用；必要文件默认进入 ZIP，可选包内／外部文件按本次选择。GitHub 独有的选中资源直接进入 ZIP，不新增本地 Skill 副本。

## 验证与局限

- [Windows 检查 run40306926235](https://github.com/Zerozero05/Skill_Manager/actions/runs/40306926235)通过：174 项前端、382 项 Rust、TypeScript／Vite、fmt、Clippy、Windows 优化构建与候选打包。测试提交 `ace98b0d6460504072499df32e46ccd53ed6d4a6` 与候选构建 `caa94532aa9e8e57f284adc7a3de4a49dd92e3d7` 的源码树相同。
- 三主题、14／22px、80%／160%缩放、1000／600／320px容器共36组实际布局检查通过，无横向溢出或列表行重叠，显示切换未产生业务请求；18px／100%另行视觉核对。
- 原生 Windows 实测覆盖逐行列表、全选和批量恢复、菜单、取消改名回焦点、暂停目录手动检查、正常关闭及完整列表滚动。候选切换时17个 Skill、139个文件摘要、16个连接及原设置独立读回一致。
- 云端仅重新打包已验 EXE，禁止打包时修改程序字节；NSIS 完整性测试、解包及内部 EXE 摘要核对通过，便携 ZIP、源码 ZIP 与成员摘要分别核验。发行源码提交以 `SOURCE_MANIFEST.json` 为准；后续差异仅限文档和发行打包层。
- 缓存说明约129ms、文件约128ms及自动导入约914ms来自此前同机合成样例，包含测量工具开销；不代表首次联网、VPN、其他电脑或所有指标改善。本次列表改动不新增后台扫描或网络请求。
- 未实际验证安装向导、真实私有仓库写入、三个客户端模型调用或其他电脑；这些不因构建和解包成功被视为通过。

## 许可与来源

项目自身尚未指定开源许可证，公开源码不等于授予 MIT 等额外使用许可。第三方许可随程序保留，见 [THIRD_PARTY_NOTICES](https://github.com/Zerozero05/Skill_Manager/blob/v0.2.10/docs/THIRD_PARTY_NOTICES.txt) 与[来源记录](https://github.com/Zerozero05/Skill_Manager/blob/v0.2.10/docs/SOURCES.md)。本次列表优化和发行准备复用已有组件与官方打包工具，没有复制新的第三方管理器代码。
