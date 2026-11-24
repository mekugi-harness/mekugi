package launch

import (
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"
	"testing"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/executionobservation"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessioncontrol"
	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionreview"
	previewbinding "github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/codexbinding"
	previewview "github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/terminalview"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/usage/accounting"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/frontends"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/command"
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/module"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
	"github.com/mekugi-harness/mekugi/mekugi-tui/testsupport/moduletest"
)

type proxyRegistryFixture struct {
	once      sync.Once
	registry  *frontends.Registry
	directory string
	err       error
}

var proxyTestFixture proxyRegistryFixture

const routerTestWorkerEnvironment = "MEKUGI_ROUTER_TEST_WORKER"
const routerTestWorkerUnscopedEnvironment = "MEKUGI_ROUTER_TEST_WORKER_UNSCOPED"

var testThreadSequence atomic.Uint64

// uniqueTestThread names a thread no other run in this process uses. The shared
// registry keeps one replay store per process, so a fixed thread ID would see an
// earlier -count run's handles and fork points.
func uniqueTestThread(prefix string) string {
	return fmt.Sprintf("%s-%d", prefix, testThreadSequence.Add(1))
}

func sharedProxyTestRegistry(t *testing.T) *frontends.Registry {
	t.Helper()
	return proxyTestFixture.get(t)
}

func (fixture *proxyRegistryFixture) get(t *testing.T) *frontends.Registry {
	t.Helper()
	fixture.once.Do(func() {
		fixture.directory, fixture.err = os.MkdirTemp("", "mekugi-proxy-tests-")
		if fixture.err != nil {
			return
		}
		if _, fixture.err = openMekugiReplayStore(filepath.Join(fixture.directory, "replay")); fixture.err != nil {
			return
		}
		fixture.registry, fixture.err = frontends.New(t.Context(), frontends.Config{Commands: testCommands(), ReplayDirectory: filepath.Join(fixture.directory, "replay")})
	})
	if fixture.err != nil {
		t.Fatal(fixture.err)
	}
	return fixture.registry
}

func newProxyWithSharedTestRegistry(t *testing.T, registry *frontends.Registry) *observationFixture {
	t.Helper()
	proxy := newMekugiProxy(registry, newTestUsage(t), testHistory())
	t.Cleanup(func() {
		if err := proxy.Close(); err != nil {
			t.Error(err)
		}
	})
	return proxy
}

func testCommands() []command.Contribution {
	hist := testHistory()
	chg, err := changehistory.Module.New(changehistory.Deps{History: hist}, module.Slots{})
	if err != nil {
		panic(err)
	}
	return []command.Contribution{moduletest.BindCommand(changehistory.Module.Contributes.Commands[0], chg)}
}

func runToolPluginWorkerInHostProcess(service *history.Service, commands []command.Contribution, ctx context.Context, argv0 string, args []string, stdin *os.File, stdout, stderr io.Writer) (bool, int) {
	return runToolPluginWorker(service, commands, command.HostProcessGroup, ctx, argv0, args, stdin, stdout, stderr)
}

func runToolPluginWorker(service *history.Service, commands []command.Contribution, processScope command.ProcessScope, ctx context.Context, argv0 string, args []string, stdin *os.File, stdout, stderr io.Writer) (bool, int) {
	invocation, err := frontends.Recognize(argv0)
	if err != nil {
		fmt.Fprintln(stderr, err)
		return true, 1
	}
	if invocation == nil {
		return false, 0
	}
	return true, invocation.Run(service, commands, processScope, ctx, args, stdin, stdout, stderr)
}

// Close releases the actual owners created by request fixtures.
func (p *observationFixture) Close() error {
	if p == nil {
		return nil
	}
	p.calls.StopAdmission()
	p.calls.Close()
	p.activity.Close()
	return nil
}

func newMekugiProxy(registry *frontends.Registry, usage *accounting.Service, service *history.Service) *observationFixture {
	if registry == nil {
		return nil
	}
	proxy := &observationFixture{
		registry:      registry,
		calls:         service.Calls(),
		review:        sessionreview.New(context.Background(), nil, nil),
		usage:         usage,
		control:       new(sessioncontrol.Observations),
		previewTiming: previewbinding.Timing{Frame: previewview.PreviewFrameDelay, Unit: previewview.PreviewUnitDelay, Finish: previewview.PreviewFinishDrain},
		execWindows:   &executionobservation.Windows{},
	}
	proxy.activity = sessioncontrol.NewAudience(func(category, message string) { proxy.notice("", "", category, message) })
	return proxy
}
