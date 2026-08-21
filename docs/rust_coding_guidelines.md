# Rust Code Style Guidelines

## Instructions

1. Fully understand the user request:
    Determine whether the task involves designing data structures, implementing traits, writing macros, modeling domain logic, or organizing modules.
    Identify key constraints such as mutability needs, ownership flow, async context, interior mutability, or concurrency boundaries.

2. Put performance first
    Tradeoff security and "clean code" for maximum performance
    Write modern CPU-friendly code
    Try not to flood instruction cache
    Use cache-friendly data fetch access patterns - linear access is the best
    Consider Data-Oriented Design where appropriate
    Assume good state, good parameter values etc - use debug_assert!() to ensure good state
    Optimize for vectorized maths
    Dont use standard math library unless actually required, prefer optimized specialistic implementations (like glam, which we are using here)
    Dont write vectors of floats when you need Vec or a Matrix
    DO NOT dynamically allocate objects that will live the entire time
    In case of branches, put the more likely branch first
    NEVER use dynamic polymorphism and dynamic dispatch; when appropriate, use a static handler dispatch table instead; when possible, prefer compile-time resolutions
    Simple, compile-time runnable functions should be compile-time evaluated

3. Data structures:
    Choose between struct, enum, or newtype based on domain needs.
    prefer slice-based APIs for performance

4. Consider ownership of each field:
    Use &str vs String, slices vs vectors, Arc<T> when sharing, or Cow<'a, T> for flexible ownership.
    Model invariants explicitly using types (e.g., NonZeroU32, Duration, custom enums).
    When there are a lot of boolean flags controlling the same state, prefer enum for state machines.

5. Write my type of Rust:
    Place impl blocks immediately below the struct/enum they modify.
    Group related methods together: constructors, getters, mutation methods, domain logic, helpers.
    Provide clear constructors (new, with_capacity) where appropriate.
    DO NOT USE MAGIC NUMBERS, prefer to write a `const SOME_FIELD_VALUE: u32 = 4`;
    Use trait implementations (Display, Debug, From, Into) to simplify conversions.
    DO NOT overuse `Result<>`, instead, WRITE A LOT OF DEBUG_ASSERTS TO MAKE SURE BAD STATE NEVER HAPPENS
    DO NOT overuse `Option<>`, instead make guarantees of existing, even if in a placeholder state (a well-defined state using an enum or a const value)

6. Apply derive macros (Debug, Clone, Serialize, Deserialize, etc.) to reduce boilerplate.
    Create small, focused declarative macros to eliminate repetitive patterns.
    For procedural macros, enforce clear boundaries and predictable generated code.

7. Optimize build speed when relevant:
    On Linux, configure .cargo/config.toml to use the mold linker when appropriate.
    Use sccache to cache compiled artifacts during development.
    Minimize unnecessary dependencies and feature flags.
    Prefer cargo check during rapid iteration over cargo build.
    Split crates into lightweight workspaces to avoid monolithic rebuilds.
    Use cargo profile settings for tuned dev/release defaults.

8. Keep a maintainable module and project structure:
    Organize code into modules reflecting ownership and domain boundaries.
    DO NOT overuse generics
    pub fields are good --> DO NOT overuse private fields, unless there is actaully a need to (e.g. a field has a very specific access pattern and shouldn't be accessed by just any class that doesn't know it well)
    long variable names are good - just try to keep below 5 words when possible
    variable names should be expressive (e.g. use `time_s`, `loop_time_us` instead of generic `time`)
    DO NOT pad with spaces to keep values aligned

9. Do not bloat the codebase with special characters and unnecessary comments
    DO NOT use special characters like ──, ✔, etc
    Keep the codebase ASCII where possible
    DO NOT use any kind of emojis and other UTF-16 characters
    Write comments only when they actually have something important or non-obvious to say
    Sometimes write /// comments when appropriate and the name of struct/function isnt expressive enough, but keep them actually informative
    An example of a non-obvious good comment: "// inverse depth: clear to 0" or "// inverse depth so we use max()"

10. Use macros to create optional code when it depends on the build target (native vs web) or an engine config (e.g. DO_RESOLVE_PASS)
    Instead of branching in such cases use macros to make the binary as small as possible and avoid flooding instruction cache



## Style Guides
Do not write comments like these after making requested changes, just do the changes
// ---------------------------------------------------------------------------
// CachedDriver<D> — caching wrapper, fully generic, zero extra dispatch
// ---------------------------------------------------------------------------

Do not pad like:
const READ  = 0b00000001;
const WRITE = 0b00000010;
Just leave it like:
const READ = 0b00000001;
const WRITE = 0b00000010;

Do not end comments with a dot



Example of bad code
