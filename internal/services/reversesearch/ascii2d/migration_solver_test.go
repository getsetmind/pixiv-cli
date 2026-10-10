package ascii2d

import (
	"context"
	"sync/atomic"
	"testing"
	"time"
)

type migrationASCII2DCacheState struct {
	UserAgent      string `json:"user_agent"`
	Clearance      string `json:"clearance"`
	HasExpiry      bool   `json:"has_expiry"`
	ExpiresRFC3339 string `json:"expires_rfc3339"`
	ExpiresUnix    int64  `json:"expires_unix"`
}

func migrationASCII2DObserveState(state solverState) migrationASCII2DCacheState {
	return migrationASCII2DCacheState{UserAgent: state.userAgent, Clearance: state.clearance, HasExpiry: state.hasExpiry, ExpiresRFC3339: state.expiresAt.Format(time.RFC3339Nano), ExpiresUnix: state.expiresAt.Unix()}
}

type migrationASCII2DCacheOutcome struct {
	Stage  string                     `json:"stage"`
	State  migrationASCII2DCacheState `json:"state"`
	Error  *migrationASCII2DError     `json:"error"`
	Calls  int32                      `json:"calls"`
	Cached bool                       `json:"cached"`
	Active bool                       `json:"active"`
	Closed bool                       `json:"closed"`
}

type migrationASCII2DCacheRow struct {
	Name                        string                         `json:"name"`
	Boundary                    string                         `json:"boundary"`
	Outcomes                    []migrationASCII2DCacheOutcome `json:"outcomes"`
	SharedContextValue          string                         `json:"shared_context_value"`
	SharedCanceledWithOneWaiter bool                           `json:"shared_canceled_with_one_waiter"`
}

func migrationASCII2DObserveCache(cache *solverStateCache, stage string, state solverState, err error, calls *atomic.Int32) migrationASCII2DCacheOutcome {
	out := migrationASCII2DCacheOutcome{Stage: stage, State: migrationASCII2DObserveState(state), Error: migrationASCII2DErr(err), Calls: calls.Load()}
	if cache != nil {
		cache.mu.Lock()
		out.Cached = cache.state != nil
		out.Active = cache.active != nil
		out.Closed = cache.closed
		cache.mu.Unlock()
	}
	return out
}

func migrationASCII2DCacheRows(t *testing.T) []migrationASCII2DCacheRow {
	t.Helper()
	rows := []migrationASCII2DCacheRow{}
	add := func(name string) migrationASCII2DCacheRow {
		return migrationASCII2DCacheRow{Name: name, Boundary: "source-driven-private-solverStateCache-not-public-output", Outcomes: []migrationASCII2DCacheOutcome{}}
	}
	now := time.Date(2030, 1, 2, 3, 4, 5, 0, time.UTC)
	for _, mode := range []string{"expiry-and-invalidation", "no-expiry", "failure-not-cached"} {
		row := add(mode)
		cache := newSolverStateCache()
		fixed := now
		cache.now = func() time.Time { return fixed }
		var calls atomic.Int32
		solve := func(context.Context) (solverState, error) {
			n := calls.Add(1)
			if mode == "failure-not-cached" && n == 1 {
				return solverState{}, ErrSolverFailed
			}
			state := solverState{userAgent: "solver-agent", clearance: "clearance-fixture"}
			if mode == "expiry-and-invalidation" {
				state.hasExpiry = true
				state.expiresAt = fixed.Add(time.Hour)
			}
			return state, nil
		}
		for _, stage := range []string{"first", "second", "advance-exact-hour", "invalidate", "closed"} {
			if stage == "advance-exact-hour" {
				fixed = fixed.Add(time.Hour)
			}
			if stage == "invalidate" {
				cache.invalidate()
			}
			if stage == "closed" {
				cache.close()
			}
			state, err := cache.getOrSolve(context.Background(), solve)
			row.Outcomes = append(row.Outcomes, migrationASCII2DObserveCache(cache, stage, state, err, &calls))
		}
		rows = append(rows, row)
	}
	for _, mode := range []string{"nil-context", "nil-solve", "nil-cache", "canceled", "deadline"} {
		row := add(mode)
		cache := newSolverStateCache()
		var calls atomic.Int32
		ctx := context.Background()
		solve := solverSolveFunc(func(context.Context) (solverState, error) { calls.Add(1); return solverState{}, nil })
		switch mode {
		case "nil-context":
			ctx = nil
		case "nil-solve":
			solve = nil
		case "nil-cache":
			cache = nil
		case "canceled":
			ended, c := context.WithCancel(ctx)
			c()
			ctx = ended
		case "deadline":
			ended, c := context.WithDeadline(ctx, time.Unix(1, 0))
			defer c()
			ctx = ended
		}
		state, err := cache.getOrSolve(ctx, solve)
		row.Outcomes = append(row.Outcomes, migrationASCII2DObserveCache(cache, "get", state, err, &calls))
		rows = append(rows, row)
	}
	for _, mode := range []string{"concurrent-shared-success", "one-waiter-canceled", "all-waiters-canceled", "close-in-flight"} {
		row := add(mode)
		cache := newSolverStateCache()
		var calls atomic.Int32
		started := make(chan context.Context, 1)
		release := make(chan struct{})
		underlyingCanceled := make(chan struct{})
		solve := func(ctx context.Context) (solverState, error) {
			calls.Add(1)
			started <- ctx
			select {
			case <-release:
				return solverState{userAgent: "solver-agent", clearance: "clearance-fixture"}, nil
			case <-ctx.Done():
				close(underlyingCanceled)
				return solverState{}, ctx.Err()
			}
		}
		type result struct {
			state solverState
			err   error
		}
		first := make(chan result, 1)
		second := make(chan result, 1)
		firstCtx, cancelFirst := context.WithCancel(context.WithValue(context.Background(), migrationASCII2DContextKey{}, "shared-context-fixture"))
		defer cancelFirst()
		go func() { state, err := cache.getOrSolve(firstCtx, solve); first <- result{state, err} }()
		sharedContext := <-started
		row.SharedContextValue, _ = sharedContext.Value(migrationASCII2DContextKey{}).(string)
		waitForSolverWaiters(t, cache, 1)
		if mode == "concurrent-shared-success" || mode == "one-waiter-canceled" {
			secondCtx, cancelSecond := context.WithCancel(context.Background())
			defer cancelSecond()
			go func() { state, err := cache.getOrSolve(secondCtx, solve); second <- result{state, err} }()
			waitForSolverWaiters(t, cache, 2)
			if mode == "one-waiter-canceled" {
				cancelSecond()
				r := <-second
				row.Outcomes = append(row.Outcomes, migrationASCII2DObserveCache(cache, "canceled-second", r.state, r.err, &calls))
				row.SharedCanceledWithOneWaiter = sharedContext.Err() != nil
				close(release)
				r = <-first
				row.Outcomes = append(row.Outcomes, migrationASCII2DObserveCache(cache, "remaining-first", r.state, r.err, &calls))
			} else {
				close(release)
				r := <-first
				row.Outcomes = append(row.Outcomes, migrationASCII2DObserveCache(cache, "first", r.state, r.err, &calls))
				r = <-second
				row.Outcomes = append(row.Outcomes, migrationASCII2DObserveCache(cache, "second", r.state, r.err, &calls))
			}
		} else {
			cache.mu.Lock()
			done := cache.active.done
			cache.mu.Unlock()
			if mode == "close-in-flight" {
				cache.close()
			} else {
				cancelFirst()
			}
			<-underlyingCanceled
			<-done
			r := <-first
			row.Outcomes = append(row.Outcomes, migrationASCII2DObserveCache(cache, "ended-first", r.state, r.err, &calls))
			replacement := func(context.Context) (solverState, error) {
				calls.Add(1)
				return solverState{userAgent: "replacement-agent", clearance: "replacement-clearance"}, nil
			}
			state, err := cache.getOrSolve(context.Background(), replacement)
			row.Outcomes = append(row.Outcomes, migrationASCII2DObserveCache(cache, "replacement", state, err, &calls))
		}
		cache.close()
		rows = append(rows, row)
	}
	return rows
}
