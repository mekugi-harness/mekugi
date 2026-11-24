# Test performance measurements

Run commands from the repository root using the [build and test](development.md#build-and-test) runner.
Select one package and pass `go test` profiling flags directly:

```sh
just test ./mekugi-tui/apps/mekugi/internal/launch -run '^TestAppServer' \
  -cpuprofile=/tmp/mekugi-launch.cpu -blockprofile=/tmp/mekugi-launch.block \
  -o=/tmp/mekugi-launch.test
```

Inspect profiles with `go tool pprof`; summed blocked time is not wall time.
