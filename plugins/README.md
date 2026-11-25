# 墨页书源插件

书源插件是一个独立运行的程序。墨页负责书库界面、TXT、EPUB 和 PDF 解析、去重和导入。插件负责账户连接、列出目录和下载选中的文件。

## 构建和安装文件夹书源

1. 在仓库根目录运行 `cargo build --release --locked -p moye-folder-source`。
2. 在墨页的「插件」页点「打开插件文件夹」。在里面建一个 `folder-source` 子目录，放入 [plugin.json](folder-source/plugin.json) 和生成的 `target/release/moye-folder-source`。Windows 使用 `moye-folder-source.exe`。
3. 回到「插件」页点「刷新」，打开「文件夹书源 → 设置」，选择存放书籍的文件夹。可指定本地文件夹或由 OneDrive 客户端同步的目录。
4. 从书库进入「书籍来源」，打开「文件夹书源」，选择一本 TXT、EPUB 或 PDF 导入。

插件文件夹默认在墨页的数据目录下。有 `MOYE_DATA_DIR` 时使用其下的 `plugins`。移除插件目录即可卸载。插件以当前系统用户身份运行，请安装可信来源的可执行文件。

## 协议 1

每个插件目录含 `plugin.json` 和对应平台的可执行文件。清单指定 `protocol`、`id`、`name`、`executable` 和 `capabilities`。用小写 ASCII 字母、数字、连字符或下划线作为 `id` 和 `executable`。清单里的 `executable` 写程序的基础名称，Windows 上由墨页加 `.exe`。

每次调用启动一个进程。进程从标准输入读取一份 UTF-8 JSON 请求，然后从标准输出返回逐行 JSON 事件并退出。工作目录是插件安装目录。不同调用之间可用 `data_dir` 保存插件自己的设置，访问令牌应交给操作系统的凭据存储。

```json
{"protocol":1,"data_dir":"/path/to/plugin-data/example","output_dir":"/path/to/temporary/books","method":"source.list","params":{"folder":null,"cursor":null,"query":null,"limit":100}}
```

| 能力 | 参数 | 成功结果 |
| --- | --- | --- |
| `source.list` | `folder`、`cursor`、`query`、`limit` | `items`、`next_cursor` 和可选的 `needs_connection` |
| `source.fetch` | `ids`，选中的书籍标识数组 | `files`，和请求顺序一致的文件名数组 |
| `source.configure` | 无 | `null`。可以打开系统浏览器完成授权 |
| `source.disconnect` | 无 | `null`。清除插件持有的连接凭据 |
| `settings.directory.get` | 无 | `{"directory":null}` 或带有目录绝对路径的同结构对象 |
| `settings.directory.set` | `directory`，由系统选择器返回的目录绝对路径 | 保存后的目录，用与 `settings.directory.get` 相同的结构返回 |

书源面板要求声明 `source.list` 和 `source.fetch`。插件设置页根据可选能力显示连接和断开按钮。声明两项目录设置能力后，墨页会显示文件夹选择器，并由插件保存用户选择。首版界面按单本导入，协议支持批量请求。列表预留了 `query` 字段，当前界面传 `null`。

- `folder: null` 表示根目录。目录和书籍的 `id` 由插件定义，墨页按原值传回。下一页使用插件返回的不透明 `next_cursor`。
- 列表每页最多 100 项。每项含 `id`、`name`、`kind` 和可选的 `size`（字节）。`kind` 为 `folder` 或 `book`。未登录时可以返回空列表和 `needs_connection: true`。
- 下载时把文件写入 `output_dir`，成功响应只返回保留 `.txt`、`.epub` 或 `.pdf` 扩展名的普通文件名。每本书上限 256 MB。墨页校验结果并在导入结束后清理临时目录。

每行事件以换行符结束。进度事件可以发多次，最后发一个结果或错误。进度的 `completed` 和 `total` 可省略。

```jsonl
{"type":"progress","message":"正在下载","completed":40,"total":100}
{"type":"result","value":{"files":["西游记.epub"]}}
```

```jsonl
{"type":"error","message":"账户授权已过期，请重新连接。"}
```

关闭书源面板或返回来源列表时，墨页会终止正在运行的请求。目录和连接操作最长 3 分钟，下载最长 15 分钟。插件应在最终结果写出前完成文件写入，成功返回结果后以状态码 0 退出。标准输出累计上限为 8 MiB。

Rust 插件可直接使用 [moye-plugin-api](sdk/src/lib.rs) 的协议类型，也可用其他语言实现相同的 JSON 协议。清单可以包含新增的能力名。墨页在实现对应界面后才会调用这些能力，已有书源可以继续按原清单运行。
