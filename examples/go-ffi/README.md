# vldb-lancedb Go FFI 示例

这个示例演示如何在 **Go** 中通过 **动态库 + 非 JSON FFI 主接口** 使用 `vldb-lancedb`。

## 目标

示例覆盖以下能力：

- 加载 `vldb_lancedb.dll/.so/.dylib`
- 创建 `LanceDbRuntime`
- 打开默认库引擎
- 使用 JSON 兼容层建表
- 使用非 JSON 主接口执行向量写入
- 使用非 JSON 主接口执行向量检索
- 读取检索结果元信息与原始结果载荷

## 运行前提

1. 已构建 `vldb-lancedb` 动态库
2. 将动态库路径传给示例程序
3. 当前系统已安装 Go 1.24+

## 运行示例

Windows:

```powershell
go run . D:\projects\VulcanLocalDataGateway\vldb-lancedb\target\debug\vldb_lancedb.dll
```

Linux:

```bash
go run . /path/to/libvldb_lancedb.so
```

macOS:

```bash
go run . /path/to/libvldb_lancedb.dylib
```

## 说明

- 这个示例刻意采用：
  - **建表**：先走 JSON 兼容层（低频 DDL）
  - **写入 / 检索**：走非 JSON 主接口（高频向量路径）
- 如果后续要给生产 Go 项目接入，建议把 `lancedbffi` 目录抽成独立内部包，再补：
  - Arrow IPC 读取
  - JSON Rows 的强类型反序列化
  - 更完整的错误码与资源托管
