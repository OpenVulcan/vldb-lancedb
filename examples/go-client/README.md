# Go gRPC link test demo

## 1) Generate Go stubs

Run these commands from the project root:

```bash
go install google.golang.org/protobuf/cmd/protoc-gen-go@v1.36.11
go install google.golang.org/grpc/cmd/protoc-gen-go-grpc@v1.6.1

protoc \
  -I . \
  --go_out=./examples/go-client/gen \
  --go_opt=paths=source_relative \
  --go-grpc_out=./examples/go-client/gen \
  --go-grpc_opt=paths=source_relative \
  ./proto/v1/lancedb.proto
```

If `protoc-gen-go` or `protoc-gen-go-grpc` is not found, add your Go bin directory to `PATH` first.

## 2) Start the Rust service

```bash
cp ./vldb-lancedb.json.example ./vldb-lancedb.json
cargo run
```

## 3) Run the Go demo

```bash
cd ./examples/go-client
go mod tidy
go run .
```

The demo now runs this full flow against the gRPC service:

1. `CreateTable`
2. `VectorUpsert`
3. `VectorSearch`
4. `Delete`
5. `DropTable`
