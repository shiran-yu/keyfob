# keyfob（钥匙扣）

给 Claude Code、你的 shell 和脚本用的 API token 管理工具。

- **交给操作系统保管**：Mac 存进钥匙串；有桌面的 Linux 存进系统密码库；服务器、集群上存进一个只有你能读的文件（权限 `0600`，所在目录 `0700`）。最后这种和 Claude Code 自己在 Linux 上存登录凭证的方式一样。
- **只给需要的那一个命令**：`keyfob run openai -- python3 eval.py`。不会导出到 shell 环境里，所以别的程序看不到，模型也看不到。
- **直接在对话框里粘贴**：给 Claude Code 发 `keyfob: s2-key <token>`，插件会在这条消息被记录之前先把 token 存好，模型和对话记录里只剩 `keyfob: s2-key [stored in keyfob]`。即使没写 `keyfob:` 前缀，只要认得出来（OpenAI、Anthropic、GitHub、Hugging Face、AWS、Slack、Google、Stripe、JWT），也会先存成 `pasted-1`。
- **Claude 的 Bash 护栏**：遇到下面这些命令会拒绝，并告诉你正确做法：
  - 会把密钥打印出来的（`keyfob get`、`env`、`echo $TOKEN`、直接读存储文件）；
  - 命令里直接写着 token 的；
  - 需要某组密钥、却没用 `keyfob run` 包起来的。

  它会先按 shell 的规则把命令拆开再判断，所以 grep 引号里的字不会被误当成命令。
- **由插件声明需求**：插件在 `keyfob.json` 里写明自己需要哪些 key，keyfob 自动找到。`/keyfob` 面板显示缺哪个、去哪里申请；`keyfob check --live` 会逐个问服务方「这个 key 还有效吗」。

用 Rust 写成，是单个程序，支持 Mac（Apple 芯片、Intel）和 Linux（x86_64、aarch64，静态编译）。

## 安装

作为 Claude Code 插件安装。程序会随插件在第一次运行时下载，并和发布页上的 SHA-256 校验值核对：

```
/plugin marketplace add yushiran/keyfob
/plugin install keyfob@keyfob
```

单独使用（服务器、脚本）：从 [Releases](https://github.com/yushiran/keyfob/releases) 下载 `keyfob-<平台>.tar.gz` 并核对 `.sha256`，或者用 `cargo install --git https://github.com/yushiran/keyfob`。

## 用法

```sh
keyfob add openai-key --env OPENAI_API_KEY      # 终端里不回显地输入；或在 Claude Code 里粘贴 `keyfob: openai-key <token>`
keyfob run openai-key -- python3 eval.py        # 脚本里读 os.environ["OPENAI_API_KEY"]
keyfob run binance -- binance-cli spot ping     # 插件声明的「组」：一次给多个变量
keyfob ls                                       # 名字、变量、谁需要它；不显示值
keyfob check --live                             # 齐不齐？服务方还认不认？
keyfob doctor                                   # 现在用哪种存储；shell 配置和 Claude 设置里还有没有明文密钥
```

存储方式可以用 `KEYFOB_BACKEND`，或在 `~/.config/keyfob/config.json` 里写 `"backend"` 指定，可选 `keychain`、`secret-service`、`file`、`auto`。

在 Mac 上，keyfob 调用的是苹果自带的 `/usr/bin/security`。这样有两个好处：
- 钥匙串的访问授权不会因为 keyfob 升级而失效，不会每次升级都弹窗问「是否允许访问」。
- 密钥只在进程之间的管道里传递，不会出现在命令行参数里。

## 给插件作者：keyfob.json

放在插件根目录。keyfob 会自动读取所有已启用插件的声明；个人自己的声明放在 `~/.config/keyfob/declarations.d/`。格式定义见 [`plugins/keyfob/schema/keyfob.schema.json`](plugins/keyfob/schema/keyfob.schema.json)，示例见英文 README。

## 能防什么、不能防什么

- **能做到**：token 不出现在 shell 配置文件、设置文件、每个进程的环境变量和你与模型的对话里；粘贴进来的 token 和 Bash 命令里的 token，也不会出现在 Claude Code 写到硬盘上的对话记录里。
- **护栏是安全带，不是沙箱**：它防的是失误。专门写来绕过它的命令是绕得过去的；读不懂输入时它会直接放行。
- **以你的身份运行的程序，能读到你能读的东西**：服务器上的存储文件防得住其他用户，防不住管理员（root）；Mac 上其他程序读钥匙串之前会弹窗问你。
- **粘贴截获的边界**：粘贴的 token 会在消息进入对话之前被存好、被替换。但 Claude Code 自己的输入历史文件（`~/.claude/history.jsonl`）是终端写的，可能不在保护范围内。请用一个假 token 实测确认；要紧的 token 请用 `/keyfob` 面板里的输入框，那里的内容根本不会变成一条消息。

## 开发

- 测试：`cargo test`（在 Mac 上加 `KEYFOB_TEST_KEYCHAIN=1` 会多跑一组钥匙串存取测试）、`claude plugin validate plugins/keyfob`、`claude plugin test plugins/keyfob`。
- 在源码目录里运行时，启动脚本直接用 `target/release` 或 `target/debug` 里编译好的程序。
- 发版：同时改 `Cargo.toml`、`plugin.json` 里的 `version` 和 `plugins/keyfob/bin/keyfob` 里的 `VERSION=`，然后推一个 `v<版本号>` 标签，GitHub 会自动编译四个平台并发布。

许可证：[CC BY-NC-SA 4.0](LICENSE)（署名、非商业性使用、相同方式共享）。0.1.0 版曾以 MIT 发布。
