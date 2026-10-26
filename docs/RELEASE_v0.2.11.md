# v0.2.11

Skill Manager 是面向Windows的本地与GitHub Agent Skills管理器：每个Skill保留一份运行文件，客户端通过目录链接共同使用。本版修正完成信息重复占据变化页、旧自动范围与脚本引用误判，并精简选择区。

## 本次变化

1. 已核对的说明／管理信息更新按“内容一致”显示并退出待办。最近检查只统计本轮新变化，重复检查不重复累计；真实修改、必要缺失、来源问题及错误继续保留。
2. 仅在可信目录清单、完整引用闭包、新鲜两端逐项一致及记录确属旧自动范围时，修正Hatch Pet的已知旧自动范围28→23项；保护人工范围和原文件，不推定历史安装提交。
3. 脚本路径与参数分别识别，保留真实中文／空格文件名，避免有效脚本被误报缺失。读取失败仍保持已有管理状态。
4. 变化页默认隐藏勾选框与长选择条；单项直接重新检查，多项按需批量选择。选择、数量和执行分组对齐，窄分栏自动换行，保留Ctrl+A、Esc、清空／退出和分页／筛选选择。

## 下载选择

| 附件 | 用途 |
| --- | --- |
| `Skill_Manager_v0.2.11_windows_x64_portable.zip` | 解压后运行`skill-manager.exe`；附使用指南、第三方许可和路径侧文件。 |
| `Skill_Manager_v0.2.11_windows_x64_setup.exe` | Windows x64 NSIS安装器；包内容核验与安装向导运行分别记录。 |
| `Skill_Manager_v0.2.11_source.zip` | 本版完整源码、锁文件、测试、说明与构建配置。 |
| `SOURCE_MANIFEST.json` | 源码成员摘要、精确源码提交、候选来源与安装器内部程序核验信息。 |
| `SHA256SUMS.txt` | 核对其余六项附件的SHA256。 |
| `USER_GUIDE.md` | 使用与故障处理指南。 |
| `RELEASE_NOTES.md` | 本发行说明。 |

[发行页与附件](https://github.com/Zerozero05/Skill_Manager/releases/tag/v0.2.11) · [公开仓库](https://github.com/Zerozero05/Skill_Manager)

便携包与安装器使用同一份已验EXE，程序SHA256为`1ca90747eca2ac3229762928d05036724229e910463c28be40925e9b2fb2092f`。需要现有Windows x64与Microsoft Edge WebView2 Runtime；附件不包含个人数据库、令牌、私人Skill或本机维护资料。

## 升级与兼容

- 正常退出旧版后替换程序文件；保留原管理数据、路径侧文件及Skill原位置。便携包默认`data-directory.txt`为`.`；既有用户沿用自己的侧文件、`--data-dir`或环境变量，不用通用默认值覆盖现有路径。
- 没有新增依赖或数据库格式迁移；原分类、收藏、来源、连接和人工文件范围保持，新增摘要字段兼容旧记录。未知安装历史继续显示未知。
- 文件范围是用于检查、更新和上传的管理记录，不是Skill运行开关。实际执行仍取决于真实文件、客户端和运行环境。
- 下载、更新和提交继续预览并手动应用。必要文件默认进入ZIP，可选包内／外部文件按本次选择；选中的GitHub独有资源直接进入ZIP，不在本地Skill目录生成副本。
- 三主题、字号与缩放、多标签／独立窗口、目录、分类收藏和客户端连接沿用现有行为。

## 验证与局限

- [Windows CI run40380368432](https://github.com/Zerozero05/Skill_Manager/actions/runs/40380368432)通过183项前端、389项Rust、TypeScript/Vite、rustfmt、Clippy（警告视为错误）、Windows优化构建及候选打包。测试源码`10e4e2c25a54f9d8d78a94fa87b83742ef132dbd`与构建提交`fa585966f21a20db298ba3410c59209a85cec180`的源码树一致；后续仅定稿发行文档。
- 原生Windows实测覆盖默认单项布局、直接检查、Ctrl+A、清空／退出、Esc、宽窄分栏和正常重启。最终单项检查约3.184秒、完整重复检查约1.383秒，均错误0；说明变化不再重复进入待办，真实本地修改继续保留。18个Skill、138文件、16连接及用户设置独立核验保持。
- 打包仅复用已验EXE；便携ZIP、NSIS完整性、解包后的内部EXE及源码ZIP成员摘要分别核验，具体打包提交与结果见`SOURCE_MANIFEST.json`。安装向导未实际运行。
- 耗时属于同机观测，不保证首次联网、VPN、其他电脑或所有场景速度。多条真实待办批量由组件回归覆盖，没有修改用户数据制造夹具。
- 本轮未重新验证真实私库写入、三个客户端模型调用或其他电脑；连接正常、loader或构建通过不等于实际调用成功。

## 许可与来源

项目自身尚未指定开源许可证，公开源码不等于授予MIT等额外许可。第三方许可随程序保留，见[第三方许可](https://github.com/Zerozero05/Skill_Manager/blob/v0.2.11/docs/THIRD_PARTY_NOTICES.txt)和[来源记录](https://github.com/Zerozero05/Skill_Manager/blob/v0.2.11/docs/SOURCES.md)。本版沿用既有组件与官方打包工具，没有新增第三方管理器代码。
