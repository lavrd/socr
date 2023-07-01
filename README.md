# Socr


## Threads

(3)

- 2 threads for TCP server
  - 1 thread for epoll events handling
  - 1 thread for general events handling (broadcast/read)
- 1 thread for consensus events handling
