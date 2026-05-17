package main

type ServerConfig struct {
    Host string
    Port int
}

type DatabaseConfig struct {
    URL string
}

type CacheConfig struct {
    TTL int
}

func main() {
    s := ServerConfig{Host: "localhost", Port: 8080}
    _ = s
}
