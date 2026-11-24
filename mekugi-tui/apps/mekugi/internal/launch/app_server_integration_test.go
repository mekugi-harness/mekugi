package launch

func testAppServerIntegration(p *observationFixture) appServerIntegration {
	if p == nil {
		return appServerIntegration{}
	}
	integration := appServerIntegration{controlObservations: p.control, usage: p.usage, audience: p.activity, review: p.review, execWindows: p.execWindows, nativeTrace: p.nativeTrace, retained: p.retained, changeSource: p.changeSource, snapshots: p.snapshots, publishChanges: p.publishChanges, captureNotice: p.noticeSink}
	integration.preparer = p.preparer()
	if p.execWindows != nil {
		integration.execTrack = p.execWindows.Reports
	}
	return integration
}
func setTestAppServerIntegration(u *appServerUI, p *observationFixture) {
	integration := testAppServerIntegration(p)
	if integration.preparer != nil {
		u.Requests.AdmitSideFork = integration.preparer.AdmitFork
	}
	u.preparer = integration.preparer
	u.controlObservations = integration.controlObservations
	u.usage = integration.usage
	u.audience = integration.audience
	u.review = integration.review
	u.execWindows = integration.execWindows
	u.nativeTrace = integration.nativeTrace
	u.retained = integration.retained
	u.changeSource = integration.changeSource
	u.snapshots = integration.snapshots
	u.publishChanges = integration.publishChanges
	u.captureNotice = integration.captureNotice
	u.review = attachReview(u.ctx, u.review, u.retained, u.changeSource)
}
