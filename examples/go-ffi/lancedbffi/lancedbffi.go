package lancedbffi

import (
	"encoding/json"
	"errors"
	"fmt"
	"runtime"
	"unsafe"

	"github.com/ebitengine/purego"
)

// StatusCode 表示 FFI 调用状态码。
// StatusCode represents the FFI invocation status code.
type StatusCode int32

const (
	// StatusSuccess 表示调用成功。
	// StatusSuccess indicates a successful invocation.
	StatusSuccess StatusCode = 0
	// StatusFailure 表示调用失败。
	// StatusFailure indicates a failed invocation.
	StatusFailure StatusCode = 1
)

// InputFormat 表示写入载荷格式。
// InputFormat represents the upsert payload format.
type InputFormat int32

const (
	// InputFormatUnspecified 表示未指定格式。
	// InputFormatUnspecified means the format is unspecified.
	InputFormatUnspecified InputFormat = 0
	// InputFormatJSONRows 表示 JSON Rows。
	// InputFormatJSONRows means JSON Rows.
	InputFormatJSONRows InputFormat = 1
	// InputFormatArrowIPC 表示 Arrow IPC。
	// InputFormatArrowIPC means Arrow IPC.
	InputFormatArrowIPC InputFormat = 2
)

// OutputFormat 表示检索输出格式。
// OutputFormat represents the search output format.
type OutputFormat int32

const (
	// OutputFormatUnspecified 表示未指定格式。
	// OutputFormatUnspecified means the format is unspecified.
	OutputFormatUnspecified OutputFormat = 0
	// OutputFormatArrowIPC 表示 Arrow IPC。
	// OutputFormatArrowIPC means Arrow IPC.
	OutputFormatArrowIPC OutputFormat = 1
	// OutputFormatJSONRows 表示 JSON Rows。
	// OutputFormatJSONRows means JSON Rows.
	OutputFormatJSONRows OutputFormat = 2
)

// RuntimeOptions 表示运行时创建参数。
// RuntimeOptions represents runtime creation options.
type RuntimeOptions struct {
	// DefaultDBPath 表示默认数据库路径。
	// DefaultDBPath is the default database path.
	DefaultDBPath string
	// DBRoot 表示命名库根目录。
	// DBRoot is the named-database root directory.
	DBRoot string
	// ReadConsistencyIntervalMS 表示读取一致性刷新间隔。
	// ReadConsistencyIntervalMS is the read-consistency refresh interval.
	ReadConsistencyIntervalMS uint64
	// HasReadConsistencyInterval 表示是否启用读取一致性间隔。
	// HasReadConsistencyInterval reports whether the refresh interval is enabled.
	HasReadConsistencyInterval bool
	// MaxUpsertPayload 表示单次写入 payload 上限。
	// MaxUpsertPayload is the maximum upsert payload size.
	MaxUpsertPayload uintptr
	// MaxSearchLimit 表示单次检索 limit 上限。
	// MaxSearchLimit is the maximum search limit.
	MaxSearchLimit uintptr
	// MaxConcurrentRequests 表示最大并发请求数。
	// MaxConcurrentRequests is the maximum number of concurrent requests.
	MaxConcurrentRequests uintptr
}

// UpsertResult 表示非 JSON 写入结果。
// UpsertResult represents the non-JSON upsert result.
type UpsertResult struct {
	// Version 表示当前版本号。
	// Version is the current version number.
	Version uint64
	// InputRows 表示输入行数。
	// InputRows is the number of input rows.
	InputRows uint64
	// InsertedRows 表示新增行数。
	// InsertedRows is the number of inserted rows.
	InsertedRows uint64
	// UpdatedRows 表示更新行数。
	// UpdatedRows is the number of updated rows.
	UpdatedRows uint64
	// DeletedRows 表示删除行数。
	// DeletedRows is the number of deleted rows.
	DeletedRows uint64
}

// SearchResult 表示非 JSON 检索返回。
// SearchResult represents the non-JSON search return value.
type SearchResult struct {
	// Format 表示实际输出格式。
	// Format is the effective output format.
	Format OutputFormat
	// Rows 表示命中行数。
	// Rows is the number of returned rows.
	Rows uint64
	// Data 表示原始结果载荷。
	// Data is the raw result payload.
	Data []byte
}

// CreateTableResult 表示建表兼容接口结果。
// CreateTableResult represents the create-table compatibility result.
type CreateTableResult struct {
	// Success 表示建表是否成功。
	// Success reports whether table creation succeeded.
	Success bool `json:"success"`
	// Message 表示返回消息。
	// Message is the returned message.
	Message string `json:"message"`
}

// CreateTableColumn 表示建表列定义。
// CreateTableColumn represents a create-table column definition.
type CreateTableColumn struct {
	// Name 表示列名。
	// Name is the column name.
	Name string `json:"name"`
	// ColumnType 表示列类型。
	// ColumnType is the column type.
	ColumnType string `json:"column_type"`
	// VectorDim 表示向量维度。
	// VectorDim is the vector dimension.
	VectorDim uint32 `json:"vector_dim,omitempty"`
	// Nullable 表示是否可空。
	// Nullable indicates whether the column is nullable.
	Nullable bool `json:"nullable"`
}

// CreateTableRequest 表示建表兼容层请求。
// CreateTableRequest represents the compatibility create-table request.
type CreateTableRequest struct {
	// TableName 表示表名。
	// TableName is the table name.
	TableName string `json:"table_name"`
	// Columns 表示列定义列表。
	// Columns is the column definition list.
	Columns []CreateTableColumn `json:"columns"`
	// OverwriteIfExists 表示表存在时是否覆盖。
	// OverwriteIfExists indicates whether to overwrite an existing table.
	OverwriteIfExists bool `json:"overwrite_if_exists"`
}

// Library 表示已加载的动态库。
// Library represents a loaded dynamic library.
type Library struct {
	// handle 表示底层动态库句柄。
	// handle is the underlying dynamic-library handle.
	handle uintptr

	// 绑定的 FFI 函数。
	// Bound FFI functions.
	runtimeOptionsDefault      func() runtimeOptionsPod
	runtimeCreate              func(runtimeOptionsPod) unsafe.Pointer
	runtimeDestroy             func(unsafe.Pointer)
	runtimeOpenDefaultEngine   func(unsafe.Pointer) unsafe.Pointer
	runtimeOpenNamedEngine     func(unsafe.Pointer, *byte) unsafe.Pointer
	runtimeDatabasePathForName func(unsafe.Pointer, *byte) *byte
	engineCreateTableJSON      func(unsafe.Pointer, *byte) *byte
	engineVectorUpsertRaw      func(unsafe.Pointer, *byte, InputFormat, *byte, uintptr, **byte, uintptr, *upsertResultPod) int32
	engineVectorSearchF32      func(unsafe.Pointer, *byte, *float32, uintptr, uint32, *byte, *byte, OutputFormat, *byteBufferPod, *searchResultMetaPod) int32
	engineDestroy              func(unsafe.Pointer)
	bytesFree                  func(byteBufferPod)
	stringFree                 func(*byte)
	lastErrorMessage           func() *byte
	clearLastError             func()
}

// Runtime 表示 LanceDB 运行时句柄。
// Runtime represents the LanceDB runtime handle.
type Runtime struct {
	// lib 表示所属动态库。
	// lib is the owning dynamic library.
	lib *Library
	// handle 表示底层 runtime 句柄。
	// handle is the underlying runtime handle.
	handle unsafe.Pointer
}

// Engine 表示数据库引擎句柄。
// Engine represents the database engine handle.
type Engine struct {
	// lib 表示所属动态库。
	// lib is the owning dynamic library.
	lib *Library
	// handle 表示底层 engine 句柄。
	// handle is the underlying engine handle.
	handle unsafe.Pointer
}

type runtimeOptionsPod struct {
	DefaultDBPath              *byte
	DBRoot                     *byte
	ReadConsistencyIntervalMS  uint64
	HasReadConsistencyInterval uint8
	_                          [7]byte
	MaxUpsertPayload           uintptr
	MaxSearchLimit             uintptr
	MaxConcurrentRequests      uintptr
}

type upsertResultPod struct {
	Version      uint64
	InputRows    uint64
	InsertedRows uint64
	UpdatedRows  uint64
	DeletedRows  uint64
}

type searchResultMetaPod struct {
	Format     uint32
	_          [4]byte
	Rows       uint64
	ByteLength uintptr
}

type byteBufferPod struct {
	Data *byte
	Len  uintptr
	Cap  uintptr
}

// Open 加载动态库并绑定所需函数。
// Open loads the dynamic library and binds required functions.
func Open(path string) (*Library, error) {
	handle, err := openLibrary(path)
	if err != nil {
		return nil, fmt.Errorf("加载动态库失败 / failed to load dynamic library: %w", err)
	}

	lib := &Library{handle: handle}
	bind := func(target any, name string) {
		purego.RegisterLibFunc(target, handle, name)
	}

	bind(&lib.runtimeOptionsDefault, "vldb_lancedb_runtime_options_default")
	bind(&lib.runtimeCreate, "vldb_lancedb_runtime_create")
	bind(&lib.runtimeDestroy, "vldb_lancedb_runtime_destroy")
	bind(&lib.runtimeOpenDefaultEngine, "vldb_lancedb_runtime_open_default_engine")
	bind(&lib.runtimeOpenNamedEngine, "vldb_lancedb_runtime_open_named_engine")
	bind(&lib.runtimeDatabasePathForName, "vldb_lancedb_runtime_database_path_for_name")
	bind(&lib.engineCreateTableJSON, "vldb_lancedb_engine_create_table_json")
	bind(&lib.engineVectorUpsertRaw, "vldb_lancedb_engine_vector_upsert_raw")
	bind(&lib.engineVectorSearchF32, "vldb_lancedb_engine_vector_search_f32")
	bind(&lib.engineDestroy, "vldb_lancedb_engine_destroy")
	bind(&lib.bytesFree, "vldb_lancedb_bytes_free")
	bind(&lib.stringFree, "vldb_lancedb_string_free")
	bind(&lib.lastErrorMessage, "vldb_lancedb_last_error_message")
	bind(&lib.clearLastError, "vldb_lancedb_clear_last_error")

	return lib, nil
}

// Close 关闭动态库句柄。
// Close closes the dynamic-library handle.
func (lib *Library) Close() error {
	if lib == nil || lib.handle == 0 {
		return nil
	}
	err := closeLibrary(lib.handle)
	lib.handle = 0
	return err
}

// DefaultRuntimeOptions 读取默认运行时参数模板。
// DefaultRuntimeOptions reads the default runtime options template.
func (lib *Library) DefaultRuntimeOptions() RuntimeOptions {
	pod := lib.runtimeOptionsDefault()
	return RuntimeOptions{
		ReadConsistencyIntervalMS:  pod.ReadConsistencyIntervalMS,
		HasReadConsistencyInterval: pod.HasReadConsistencyInterval != 0,
		MaxUpsertPayload:           pod.MaxUpsertPayload,
		MaxSearchLimit:             pod.MaxSearchLimit,
		MaxConcurrentRequests:      pod.MaxConcurrentRequests,
	}
}

// CreateRuntime 创建运行时。
// CreateRuntime creates the runtime.
func (lib *Library) CreateRuntime(options RuntimeOptions) (*Runtime, error) {
	defaultPathPtr, keepDefault := makeOptionalCString(options.DefaultDBPath)
	defer keepDefault()
	dbRootPtr, keepRoot := makeOptionalCString(options.DBRoot)
	defer keepRoot()
	pod := runtimeOptionsPod{
		DefaultDBPath:              defaultPathPtr,
		DBRoot:                     dbRootPtr,
		ReadConsistencyIntervalMS:  options.ReadConsistencyIntervalMS,
		HasReadConsistencyInterval: boolToUint8(options.HasReadConsistencyInterval),
		MaxUpsertPayload:           options.MaxUpsertPayload,
		MaxSearchLimit:             options.MaxSearchLimit,
		MaxConcurrentRequests:      options.MaxConcurrentRequests,
	}
	handle := lib.runtimeCreate(pod)
	if handle == nil {
		return nil, lib.lastError()
	}
	rt := &Runtime{lib: lib, handle: handle}
	runtime.SetFinalizer(rt, func(runtimeHandle *Runtime) {
		_ = runtimeHandle.Close()
	})
	return rt, nil
}

// Close 销毁运行时句柄。
// Close destroys the runtime handle.
func (rt *Runtime) Close() error {
	if rt == nil || rt.handle == nil {
		return nil
	}
	rt.lib.runtimeDestroy(rt.handle)
	rt.handle = nil
	runtime.SetFinalizer(rt, nil)
	return nil
}

// OpenDefaultEngine 打开默认数据库引擎。
// OpenDefaultEngine opens the default database engine.
func (rt *Runtime) OpenDefaultEngine() (*Engine, error) {
	handle := rt.lib.runtimeOpenDefaultEngine(rt.handle)
	if handle == nil {
		return nil, rt.lib.lastError()
	}
	engine := &Engine{lib: rt.lib, handle: handle}
	runtime.SetFinalizer(engine, func(engineHandle *Engine) {
		_ = engineHandle.Close()
	})
	return engine, nil
}

// OpenNamedEngine 按名称打开数据库引擎。
// OpenNamedEngine opens a named database engine.
func (rt *Runtime) OpenNamedEngine(name string) (*Engine, error) {
	namePtr, keep := makeOptionalCString(name)
	defer keep()
	handle := rt.lib.runtimeOpenNamedEngine(rt.handle, namePtr)
	if handle == nil {
		return nil, rt.lib.lastError()
	}
	engine := &Engine{lib: rt.lib, handle: handle}
	runtime.SetFinalizer(engine, func(engineHandle *Engine) {
		_ = engineHandle.Close()
	})
	return engine, nil
}

// DatabasePathForName 解析命名库的路径。
// DatabasePathForName resolves the path for a named database.
func (rt *Runtime) DatabasePathForName(name string) (string, error) {
	namePtr, keep := makeOptionalCString(name)
	defer keep()
	return rt.lib.takeOwnedString(func() *byte {
		return rt.lib.runtimeDatabasePathForName(rt.handle, namePtr)
	})
}

// Close 销毁引擎句柄。
// Close destroys the engine handle.
func (engine *Engine) Close() error {
	if engine == nil || engine.handle == nil {
		return nil
	}
	engine.lib.engineDestroy(engine.handle)
	engine.handle = nil
	runtime.SetFinalizer(engine, nil)
	return nil
}

// CreateTable 使用 JSON 兼容层建表。
// CreateTable uses the JSON compatibility layer to create a table.
func (engine *Engine) CreateTable(request CreateTableRequest) (CreateTableResult, error) {
	payload, err := json.Marshal(request)
	if err != nil {
		return CreateTableResult{}, fmt.Errorf("序列化建表请求失败 / failed to marshal create-table request: %w", err)
	}
	ptr, keep := makeCStringBytes(payload)
	defer keep()
	raw, err := engine.lib.takeOwnedString(func() *byte {
		return engine.lib.engineCreateTableJSON(engine.handle, ptr)
	})
	if err != nil {
		return CreateTableResult{}, err
	}
	var result CreateTableResult
	if err := json.Unmarshal([]byte(raw), &result); err != nil {
		return CreateTableResult{}, fmt.Errorf("解析建表响应失败 / failed to unmarshal create-table response: %w", err)
	}
	return result, nil
}

// VectorUpsertRaw 执行非 JSON 向量写入。
// VectorUpsertRaw executes non-JSON vector upsert.
func (engine *Engine) VectorUpsertRaw(tableName string, format InputFormat, data []byte, keyColumns []string) (UpsertResult, error) {
	tablePtr, keepTable := makeCString(tableName)
	defer keepTable()
	var dataPtr *byte
	var dataLen uintptr
	if len(data) > 0 {
		dataPtr = &data[0]
		dataLen = uintptr(len(data))
	}
	keyPtrs, keepKeys := makeCStringArray(keyColumns)
	defer keepKeys()
	var pod upsertResultPod
	status := StatusCode(engine.lib.engineVectorUpsertRaw(
		engine.handle,
		tablePtr,
		format,
		dataPtr,
		dataLen,
		keyPtrs,
		uintptr(len(keyColumns)),
		&pod,
	))
	if status != StatusSuccess {
		return UpsertResult{}, engine.lib.lastError()
	}
	return UpsertResult{
		Version:      pod.Version,
		InputRows:    pod.InputRows,
		InsertedRows: pod.InsertedRows,
		UpdatedRows:  pod.UpdatedRows,
		DeletedRows:  pod.DeletedRows,
	}, nil
}

// VectorSearchF32 执行非 JSON 向量检索。
// VectorSearchF32 executes non-JSON vector search.
func (engine *Engine) VectorSearchF32(tableName string, vector []float32, limit uint32, filter string, vectorColumn string, outputFormat OutputFormat) (SearchResult, error) {
	if len(vector) == 0 {
		return SearchResult{}, errors.New("vector must not be empty / 向量不能为空")
	}
	tablePtr, keepTable := makeCString(tableName)
	defer keepTable()
	filterPtr, keepFilter := makeOptionalCString(filter)
	defer keepFilter()
	vectorColumnPtr, keepVectorColumn := makeOptionalCString(vectorColumn)
	defer keepVectorColumn()
	var buffer byteBufferPod
	var meta searchResultMetaPod
	status := StatusCode(engine.lib.engineVectorSearchF32(
		engine.handle,
		tablePtr,
		&vector[0],
		uintptr(len(vector)),
		limit,
		filterPtr,
		vectorColumnPtr,
		outputFormat,
		&buffer,
		&meta,
	))
	if status != StatusSuccess {
		return SearchResult{}, engine.lib.lastError()
	}
	defer engine.lib.bytesFree(buffer)
	var payload []byte
	if buffer.Data != nil && buffer.Len > 0 {
		payload = append([]byte(nil), unsafe.Slice(buffer.Data, buffer.Len)...)
	}
	return SearchResult{
		Format: OutputFormat(meta.Format),
		Rows:   meta.Rows,
		Data:   payload,
	}, nil
}

// takeOwnedString 调用返回 char* 的函数并自动释放返回值。
// takeOwnedString calls a function that returns char* and frees the returned value automatically.
func (lib *Library) takeOwnedString(getter func() *byte) (string, error) {
	ptr := getter()
	if ptr == nil {
		return "", lib.lastError()
	}
	defer lib.stringFree(ptr)
	return readCString(ptr), nil
}

// lastError 读取最近一次错误信息。
// lastError reads the latest error message.
func (lib *Library) lastError() error {
	if lib == nil {
		return errors.New("library is nil / library 不能为空")
	}
	ptr := lib.lastErrorMessage()
	if ptr == nil {
		return errors.New("ffi call failed without error message / FFI 调用失败但未返回错误消息")
	}
	return errors.New(readCString(ptr))
}

// makeCString 创建临时 C 风格字符串缓冲区。
// makeCString creates a temporary C-style string buffer.
func makeCString(value string) (*byte, func()) {
	buffer := append([]byte(value), 0)
	return &buffer[0], func() {
		runtime.KeepAlive(buffer)
	}
}

// makeCStringBytes 从 UTF-8 bytes 创建临时 C 风格字符串缓冲区。
// makeCStringBytes creates a temporary C-style string buffer from UTF-8 bytes.
func makeCStringBytes(value []byte) (*byte, func()) {
	buffer := append(append([]byte(nil), value...), 0)
	return &buffer[0], func() {
		runtime.KeepAlive(buffer)
	}
}

// makeOptionalCString 创建可选 C 风格字符串缓冲区。
// makeOptionalCString creates an optional C-style string buffer.
func makeOptionalCString(value string) (*byte, func()) {
	if value == "" {
		return nil, func() {}
	}
	return makeCString(value)
}

// makeCStringArray 创建 C 字符串数组缓冲区。
// makeCStringArray creates a C-string array buffer.
func makeCStringArray(values []string) (**byte, func()) {
	if len(values) == 0 {
		return nil, func() {}
	}
	buffers := make([][]byte, 0, len(values))
	pointers := make([]*byte, 0, len(values))
	for _, value := range values {
		buffer := append([]byte(value), 0)
		buffers = append(buffers, buffer)
		pointers = append(pointers, &buffer[0])
	}
	return &pointers[0], func() {
		runtime.KeepAlive(buffers)
		runtime.KeepAlive(pointers)
	}
}

// readCString 读取 NUL 结尾字符串。
// readCString reads a NUL-terminated string.
func readCString(ptr *byte) string {
	if ptr == nil {
		return ""
	}
	base := uintptr(unsafe.Pointer(ptr))
	length := 0
	for {
		value := *(*byte)(unsafe.Pointer(base + uintptr(length)))
		if value == 0 {
			break
		}
		length++
	}
	return string(unsafe.Slice(ptr, length))
}

// boolToUint8 将布尔值转成 FFI 所需的 0/1。
// boolToUint8 converts a boolean to the 0/1 FFI form.
func boolToUint8(value bool) uint8 {
	if value {
		return 1
	}
	return 0
}
