now that fat32 kinda works:

- basic filesystem operations --> reading, reading at n pos, writing, writing at n pos, appending

- process file descriptors

- processes with stdout --> stdout into terminal somehow (the terminal that this process has, as we are going to allow multiple) (how does linux handle this?)


REAL TODO:

- !! file caching

- get rid of dynamic allocations in file reading

- hash algorithms

- better perf testing
    repetitions

- file cache arena may have a compacting on all operations problem when it reaches the limit, try to do some smart eviction up to some threshold free space
- file cache - maybe keep a list of files sorted by their importance or something better, so we dont have to iterate and choose importance for evict_one, especially for when multiple evictions have to happen

- !!! MAX_CORES set to the actual count of the cores causes a page fault on the (MAX_CORES-1) core after entering scheduler loop now, it used to work fine

- validating user memory areas
    maybe at the granularity of the 64kB slabs of pages that i give each malloc

- now that we can redirect logs, write automated tests for cretain parts of kernel and for userspace programs
- create test suite userspace programs that will run and test stuff

- make sure loaded program sections get page-aligned and have actual proper protection flags

- thread context switching, two processes running on 1 core
- upstream bigos compositor and terminal (no not yet, we will do userspace shell, doing it kernel mode now would couple things i dont want coupled and that would be problematic in the future) to this
- enable sse builds?
- rdtscp instead of rdtsc in timer handler to get core_id

- grouping syscalls
    instead of always doing the entire ring swap, group syscalls that can be called at a single time
        the whole window creation chain is a good example

- stdin/stout routing for userspace
    I suppose we first do a userspace shell
        FIRST FIGURE OUT HOW TO HANDLE THE SHARED BUFFERS, SHELL'S ARCHITECTURE DEPENDS ON THAT
    then we define what stdout and stdin is
    possibly just IPC then, and the process can write to a dedicated shared buffer for stdout stdin???
    a dedicated shared buffer sounds fine
    maybe processes ask for access to it
    have to also handle the length of the messages that would go through shared memory
        cant just limit it to like 16 pages and call it
        a linked list of pages for that global buffer? and the list is like an arena, reusable blocks
        can use the paging ring possibly
    or its given by default
        file descriptors connected to a terminal emulator process?

    For actual no-syscall shared memory i suppose id have to go with a doorbell approach
    possibly even could let the waiting process yield, and implement a special scheduling case in kernel for doorbell-waiting processes
        well essentially blocking async io
        API includable as a part of the OS lib
    else id do a pipe, and do syscalls for write/read
        i could make a freelist page buffer like that shouldnt overflow as long as we have physical space:
            4kb pages and 64kb starting memory, working like a freelist with each page pointing to the next, until a free page. Free page should have a special marker in header, but also point to another free page. The first free page (and the last used page (the one that was given the latest) of that chain should be stored in the structure (just its index) and used by default when another page is needed. When that first free page has an invalid index (-1) (that should only happen when there are no more free pages left), allocate 4 more pages and hook up to the structure
            but also have to figure out producer/consumer sync
            also called a global allocator lmao
            maybe just let the process specify how much memory it will need at all times max and burden the process with optimizing itself
            i need extremely small overhead on all things in this OS, so maybe making the processes be mandatorily smart is the real way
                esp since nobody is ever gonna use that and id know how to write programs for my own kernel

- some regression tests for the future for the userspace stuff

- hook up the virtio_drivers crate; but there is no point for now and id have to probably maintain it when it updates; ata is fine for now

graphics:

- at this point tough to optimize anything, should do multithreaded rendering
- might also eg shade 4 pixels at once the same way, and interpolate 16
- would tiling be beneficial?
