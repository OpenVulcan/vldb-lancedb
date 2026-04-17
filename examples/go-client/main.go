package main

import (
	"context"
	"encoding/json"
	"log"
	"time"

	lancedbv1 "vldb-lancedb-go-demo/gen/proto/v1"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
)

func main() {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	conn, err := grpc.DialContext(
		ctx,
		"127.0.0.1:19301",
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithBlock(),
	)
	if err != nil {
		log.Fatalf("dial failed: %v", err)
	}
	defer conn.Close()

	client := lancedbv1.NewLanceDbServiceClient(conn)

	createResp, err := client.CreateTable(ctx, &lancedbv1.CreateTableRequest{
		TableName:         "demo_vectors",
		OverwriteIfExists: true,
		Columns: []*lancedbv1.ColumnDef{
			{Name: "id", ColumnType: lancedbv1.ColumnType_COLUMN_TYPE_INT64, Nullable: false},
			{Name: "vector", ColumnType: lancedbv1.ColumnType_COLUMN_TYPE_VECTOR_FLOAT32, VectorDim: 4, Nullable: false},
			{Name: "label", ColumnType: lancedbv1.ColumnType_COLUMN_TYPE_STRING, Nullable: true},
			{Name: "active", ColumnType: lancedbv1.ColumnType_COLUMN_TYPE_BOOL, Nullable: true},
		},
	})
	if err != nil {
		log.Fatalf("CreateTable failed: %v", err)
	}
	log.Printf("CreateTable => success=%v message=%s", createResp.Success, createResp.Message)

	rows := []map[string]any{
		{"id": 1, "vector": []float32{0.1, 0.2, 0.3, 0.4}, "label": "alpha", "active": true},
		{"id": 2, "vector": []float32{0.11, 0.19, 0.31, 0.39}, "label": "beta", "active": true},
		{"id": 3, "vector": []float32{0.9, 0.8, 0.7, 0.6}, "label": "far", "active": false},
	}
	payload, err := json.Marshal(rows)
	if err != nil {
		log.Fatalf("json marshal failed: %v", err)
	}

	upsertResp, err := client.VectorUpsert(ctx, &lancedbv1.UpsertRequest{
		TableName:   "demo_vectors",
		InputFormat: lancedbv1.InputFormat_INPUT_FORMAT_JSON_ROWS,
		Data:        payload,
		KeyColumns:  []string{"id"},
	})
	if err != nil {
		log.Fatalf("VectorUpsert failed: %v", err)
	}
	log.Printf(
		"VectorUpsert => version=%d inserted=%d updated=%d deleted=%d",
		upsertResp.Version,
		upsertResp.InsertedRows,
		upsertResp.UpdatedRows,
		upsertResp.DeletedRows,
	)

	searchResp, err := client.VectorSearch(ctx, &lancedbv1.SearchRequest{
		TableName:    "demo_vectors",
		Vector:       []float32{0.1, 0.2, 0.3, 0.4},
		Limit:        2,
		Filter:       "active = true",
		VectorColumn: "vector",
		OutputFormat: lancedbv1.OutputFormat_OUTPUT_FORMAT_JSON_ROWS,
	})
	if err != nil {
		log.Fatalf("VectorSearch failed: %v", err)
	}

	log.Printf("VectorSearch => rows=%d format=%s", searchResp.Rows, searchResp.Format)
	log.Printf("VectorSearch JSON => %s", string(searchResp.Data))

	deleteResp, err := client.Delete(ctx, &lancedbv1.DeleteRequest{
		TableName: "demo_vectors",
		Condition: "id >= 1",
	})
	if err != nil {
		log.Fatalf("Delete failed: %v", err)
	}
	log.Printf(
		"Delete => version=%d deleted=%d message=%s",
		deleteResp.Version,
		deleteResp.DeletedRows,
		deleteResp.Message,
	)

	dropResp, err := client.DropTable(ctx, &lancedbv1.DropTableRequest{
		TableName: "demo_vectors",
	})
	if err != nil {
		log.Fatalf("DropTable failed: %v", err)
	}
	log.Printf("DropTable => success=%v message=%s", dropResp.Success, dropResp.Message)
}
