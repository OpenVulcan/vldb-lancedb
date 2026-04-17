package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"

	"vldb-lancedb-go-ffi-demo/lancedbffi"
)

// demoRow 表示示例中的单条向量记录。
// demoRow represents a single vector row used by the example.
type demoRow struct {
	// ID 表示记录主键。
	// ID is the primary key of the row.
	ID string `json:"id"`
	// FilePath 表示记录对应的文件路径。
	// FilePath is the file path associated with the row.
	FilePath string `json:"file_path"`
	// Content 表示记录正文内容。
	// Content is the textual content of the row.
	Content string `json:"content"`
	// Embedding 表示向量列。
	// Embedding is the vector column.
	Embedding []float32 `json:"embedding"`
}

// searchRow 表示 JSON Rows 检索结果中的单条记录。
// searchRow represents a single hit from the JSON Rows search result.
type searchRow struct {
	// ID 表示命中的记录主键。
	// ID is the primary key of the hit row.
	ID string `json:"id"`
	// FilePath 表示命中的文件路径。
	// FilePath is the file path of the hit row.
	FilePath string `json:"file_path"`
	// Content 表示命中的正文内容。
	// Content is the textual content of the hit row.
	Content string `json:"content"`
	// Distance 表示 LanceDB 原始距离字段。
	// Distance is the raw LanceDB distance field.
	Distance float64 `json:"_distance"`
}

// main 是 Go FFI 示例入口，演示如何通过动态库接入 `vldb-lancedb`。
// main is the Go FFI demo entry point showing how to use `vldb-lancedb` through the dynamic library.
func main() {
	if len(os.Args) < 2 {
		fmt.Fprintf(os.Stderr, "用法 / Usage: %s <vldb_lancedb dynamic library path>\n", filepath.Base(os.Args[0]))
		os.Exit(2)
	}

	dynamicLibPath := os.Args[1]
	databaseRoot := filepath.Join(os.TempDir(), "vldb-lancedb-go-ffi-demo")

	if err := os.MkdirAll(databaseRoot, 0o755); err != nil {
		fmt.Fprintf(os.Stderr, "创建示例数据库目录失败 / failed to create demo database directory: %v\n", err)
		os.Exit(1)
	}

	if err := run(dynamicLibPath, databaseRoot); err != nil {
		fmt.Fprintf(os.Stderr, "Go FFI 示例执行失败 / Go FFI demo failed: %v\n", err)
		os.Exit(1)
	}
}

// run 执行完整的 LanceDB Go FFI 演示流程。
// run executes the full LanceDB Go FFI demonstration flow.
func run(dynamicLibPath string, databaseRoot string) error {
	library, err := lancedbffi.Open(dynamicLibPath)
	if err != nil {
		return err
	}
	defer func() {
		_ = library.Close()
	}()

	options := library.DefaultRuntimeOptions()
	options.DefaultDBPath = filepath.Join(databaseRoot, "default")
	options.DBRoot = filepath.Join(databaseRoot, "named")

	runtimeHandle, err := library.CreateRuntime(options)
	if err != nil {
		return err
	}
	defer func() {
		_ = runtimeHandle.Close()
	}()

	engine, err := runtimeHandle.OpenDefaultEngine()
	if err != nil {
		return err
	}
	defer func() {
		_ = engine.Close()
	}()

	createResult, err := engine.CreateTable(lancedbffi.CreateTableRequest{
		TableName: "memory",
		Columns: []lancedbffi.CreateTableColumn{
			{Name: "id", ColumnType: "string", Nullable: false},
			{Name: "file_path", ColumnType: "string", Nullable: false},
			{Name: "content", ColumnType: "string", Nullable: false},
			{Name: "embedding", ColumnType: "vector_float32", VectorDim: 4, Nullable: false},
		},
		OverwriteIfExists: true,
	})
	if err != nil {
		return err
	}
	fmt.Printf("建表结果 / CreateTable: success=%v message=%s\n", createResult.Success, createResult.Message)

	rows := []demoRow{
		{
			ID:        "demo-1",
			FilePath:  "/memory/demo-1.md",
			Content:   "向量检索示例一",
			Embedding: []float32{0.11, 0.22, 0.33, 0.44},
		},
		{
			ID:        "demo-2",
			FilePath:  "/memory/demo-2.md",
			Content:   "向量检索示例二",
			Embedding: []float32{0.19, 0.29, 0.39, 0.49},
		},
	}

	payload, err := json.Marshal(rows)
	if err != nil {
		return fmt.Errorf("序列化示例写入数据失败 / failed to marshal demo rows: %w", err)
	}

	upsertResult, err := engine.VectorUpsertRaw(
		"memory",
		lancedbffi.InputFormatJSONRows,
		payload,
		[]string{"id"},
	)
	if err != nil {
		return err
	}
	fmt.Printf(
		"写入结果 / Upsert: version=%d input=%d inserted=%d updated=%d deleted=%d\n",
		upsertResult.Version,
		upsertResult.InputRows,
		upsertResult.InsertedRows,
		upsertResult.UpdatedRows,
		upsertResult.DeletedRows,
	)

	searchResult, err := engine.VectorSearchF32(
		"memory",
		[]float32{0.10, 0.20, 0.30, 0.40},
		2,
		"",
		"embedding",
		lancedbffi.OutputFormatJSONRows,
	)
	if err != nil {
		return err
	}

	fmt.Printf(
		"检索结果元信息 / Search meta: format=%d rows=%d payload_bytes=%d\n",
		searchResult.Format,
		searchResult.Rows,
		len(searchResult.Data),
	)

	var hits []searchRow
	if err := json.Unmarshal(searchResult.Data, &hits); err != nil {
		return fmt.Errorf("解析 JSON Rows 检索结果失败 / failed to unmarshal JSON Rows search result: %w", err)
	}

	for index, hit := range hits {
		fmt.Printf(
			"命中 #%d / Hit #%d: id=%s file_path=%s distance=%.6f content=%s\n",
			index+1,
			index+1,
			hit.ID,
			hit.FilePath,
			hit.Distance,
			hit.Content,
		)
	}

	return nil
}
