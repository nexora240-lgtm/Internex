package main

import (
	"log"
	"net/http"
	"os"

	"internex/internal/transport"
)

func main() {
	port := os.Getenv("PORT")
	if port == "" {
		port = "8080"
	}

	// Set the proxy origin so URL encoding/decoding knows our address.
	host := os.Getenv("HOST")
	if host == "" {
		host = "localhost"
	}
	transport.ProxyOrigin = "http://" + host + ":" + port

	mux := transport.NewMux()

	addr := ":" + port
	log.Printf("listening on %s", addr)
	if err := http.ListenAndServe(addr, mux); err != nil {
		log.Fatalf("server error: %v", err)
	}
}
