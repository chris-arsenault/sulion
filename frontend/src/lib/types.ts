// Shared tiny type aliases. Kept in one place so rules like
// local/no-raw-undefined-union have a single named home for the
// common "value might not be set yet" pattern.

/** A value that may be `undefined`. Prefer this alias or a `?:` optional
 * shorthand over raw `T | undefined` unions at call sites. */
export type Maybe<T> = T | undefined;
