# unix-cron-schedule-iterator

Iterates forward through all instants matching a five-field Unix cron expression, starting from a given Unix timestamp.

```rust
use unix_cron_schedule_iterator::CronSchedule;

fn main() {
    // Every 15 minutes.
    let schedule = CronSchedule::parse("*/15 * * * *").expect("valid expression");

    // Start at 2024-01-01 00:00:00 UTC.
    let start = 1_704_067_200;
    let next_three: Vec<i64> = schedule.iter_from(start).take(3).collect();

    assert_eq!(next_three, vec![1_704_067_200, 1_704_068_100, 1_704_069_000]);
}
```

The crate exports two public types:

- `CronSchedule` with `parse(&str) -> Result<CronSchedule, ParseError>`, `matches(i64) -> bool`, and `iter_from(i64) -> CronIterator<'_>`.
- `CronIterator<'a>`, an `Iterator<Item = i64>`.

`ParseError` is also public and implements `std::error::Error` and `Display`; its `message()` method returns the same text as `Display`.

## Why this exists

Most cron libraries drag in a date/time crate and a parser combinator. This one does not. It parses the five numeric fields itself and converts Unix timestamps to civil dates with a small, well-known integer algorithm. The trade-off is speed on sparse schedules: the iterator advances one minute at a time, so `0 0 29 2 *` (the leap day) walks through up to four years of minutes before it finds a match. For the common case of finding the next few runs of a normal schedule, that is more than fast enough and the code stays small and auditable.

## The awkward edge

The iterator is bounded by a four-year horizon. If no match exists within four years of the starting point, `next()` returns `None`. This happens for impossible dates such as `0 0 30 2 *` (February 30th). It is also the only signal the iterator gives that the schedule is unsatisfiable; there is no error type on the iterator itself. Treat `None` as "no match within the horizon", not as "the schedule is malformed" — malformed expressions are rejected at parse time.

Timestamps are interpreted as UTC. The library does not know about time zones or daylight saving transitions; convert local times to UTC before calling `iter_from`.

## Day-of-month and day-of-week

When both the day-of-month and day-of-week fields are restricted, a date matches if either field matches. When only one is restricted, only that field is consulted. This follows the traditional Vixie cron behaviour. The fields are restricted if the source text is anything other than `*`.

## Syntax

Each of the five fields accepts `*`, a single number, a range `a-b`, a step `*/n` or `a-b/n`, and comma-separated combinations of those. Named months and weekdays are not supported. Values are validated against the usual ranges: minute 0-59, hour 0-23, day of month 1-31, month 1-12, day of week 0-6 with 0 meaning Sunday.
