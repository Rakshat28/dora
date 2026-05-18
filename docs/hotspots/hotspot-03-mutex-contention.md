# Hotspot 03: Mutex Contention on Results Accumulator

## Observed in flamegraph
Frame: `std::sync::mutex::Mutex<T>::lock`
Child frame: `pthread_mutex_lock`
Approximate share of CPU time: 5-15% on machines with 8+ cores

## Description
run_search uses Arc<Mutex<Vec<MatchResult>>> for result accumulation.
On high-core-count machines, Rayon saturates all cores with parse work,
then every thread races to lock the results Mutex to append matches.
The lock acquisition overhead is proportional to core count and match
density — worst case on a 16-core machine searching a high-density repo.

## Proposed fix
Replace the global Mutex with Rayon's fold/reduce pattern which gives each
thread a local Vec and merges at the end with zero contention during the
parallel phase. This was the original architecture before issue #24 changed
it to Mutex per the issue spec. Reverting to fold/reduce is the correct
production optimization.

## Child issue
#43c — Replace Mutex<Vec<MatchResult>> with Rayon fold/reduce for zero-contention accumulation
