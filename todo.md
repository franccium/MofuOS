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

- easier way to get file size, and sys_stat_file(fd) instead of (path)

- less allocations in odys, scratch buffer arena
    figure out how to put a file editor in it
        read file into a buffer, edit that buffer in-odys
        when save file - flush to fs (fs cache but that only kernel knows)
        

- make a rect-based rendering pipeline
    or research some real software rendering solutions

- move windows

- create and map an event buffer for each new process, in some global map of phys_addr, virt_addr for the event buffer for the given PID (outside of process manager so compositor doesnt have to lock the process manager to forward input), and read within each process from that

- launch other processes from a userspace process
    then shell integration

    to create a process i need to pass the elf
    so thats what the syscall gets, and some arguments for the process / process creation (pass: elf, name, ptr to a buffer with process start args that match what the process wants and the process can interpret itself after start)
    from shell - we would lookup the command if its a know mapped binary
    ideally, id need the process binary to live entirely on the OS-loaded disk image and pass that path
        thats ideal if i wanted to map the paths like linux, readable as files, cause it makes no sense to map a path outside of that disk image that would be too meta
        and required to load elf data for programs - for now wa is to always load the elf data for all userspace programs cause i cant dynamically load it from outside the in-OS-storage
    for now ill just start simple and map the path like i did for launching test processes
        lets do a per-process static data, that will also include a path repository for utils and programs
        which has issues with visibility of potential modifications, e.g. changing the path but which is for now fine
        right now, using a vec for path translation to be at least somewhat dynamic with the paths

    this is either:
        a seperate terminal application, where id also need to decouple shell from the terminal
        or a part of theophe
        i suppose its better to decouple shell from terminal now, allows for multiple terminal windows later and stuff
        actually as of now we dont even have a shell, theophe can be what it is, so a window that takes keyboard input, it will just need to send that input to be handled differently by different things
        so shell and interpreter would take lines entered and interpret them and its the default
        any keyboard input it would need can also be translated into commands on the terminal side, eg. ctl+c into kill -c --> kill current
        processes work within shell, shell forwards their stdout into terminal's stdin
            that falls into being another linux, but it just makes sense
        maybe its not the terminal that reads IO input, but just the apps - gets rid of one indirection - then the terminal just has a text buffer to display
        that means we will have a terminal renderer now
        and a graphics renderer, both available to apps
        and then the app is the core and i dont even think in terms of terminal
        this means apps will have to implement more themselves but thats perfectly fine

        the terminal cant be the owner of the whole backing buffer, since not all terminal apps would have a buffer thats mostly appending, and not all would have a scrollback buffer
        so i cant do a big circular scrollback for all, since e.g. file explorer and text editor would always have to display just one terminal frame, and any line can change completely at any time, while a more classic console-like terminal app would want a big scrollback and only append new lines

        however, with the approach that apps themselves need to handle stuff and terminal is just some rendering api that just happens to be for rendering text, that would be a perfect generalization, since all apps can have a buffer that fits them, and dont need to think about how to publish data for terminal to render/synchronize with it, they just have the data, format it, and render / not render, since that will also let them handle when redraw needs to happen, and a redraw has to happen very rarely, compared to regular update, in a terminal app

        and since i use embedded_graphics i dont even need to write anything now:
        just like theophe, each app will have a window it can draw to anyways, so all it needs to do is call draw(line) for each line it wants to redraw, so theophe is now a perfect reference example for apps, which can now implement their own "renderers"

        tldr: what i have now is fine, each app can be monolythic like that, what happens in the app or its child process stays in the app or its child process, and if the app want to render that something (call it stdout or w/e) it can render text calling draw(line), no terminal and terminal renderer nonsense, each app is a window app with text, or gfx, rendering capabilities

- file explorer in theophe
    like terminal file explorers

- text editor in theophe
    like terminal text editors
    integrated with the file explorer
    i suppose text editor would be a part of that, like a module used by the file explorer
    or better to think of it as an IDE

- per-cpu scratch buffer

- get rid of that DecodedKey to Keys translation, and pack events more, the keys can be u8 and i forgot why they are not right now

- file cache arena may have a compacting on all operations problem when it reaches the limit, try to do some smart eviction up to some threshold free space
- file cache - maybe keep a list of files sorted by their importance or something better, so we dont have to iterate and choose importance for evict_one, especially for when multiple evictions have to happen

- multithreaded filesystem access

- key cursor navigation in terminal
- and mouse --> we know because of how we render how many pixels each letter has, its monospace, so we can do math based on mouse pos within the window and offset of first letter from the left border to determine the cursor position
- the mouse events have to first go through compositor, as pressing outside of a window will unfocus it and bring the one in pressed area
so maybe compositor detects mouse press --> checks rects for what is it within, first for focued window: if pressed within the focsed window, send a pressed within window event to the window process, else change focused window and consume the mouse press
    easiest thing would be to have compositor consume events, and replicate them for processes, the focused process i suppose (otherwise we could have multiple processes racing for one event buffer), so the compoisitor is the primary consumer of events, and sends events further down the userspace where it thinks is appropriate 
    just stating that the process can read and handle only if its focused, makes it so that it can race the compositor to a given event that put that process out of focus etc
    render mouse cursor - a 2x2 px square at mouse x mouse y; ps2-mouse inits at 0,0, so we add deltas and store current mouse pos from last poll

- mouse support

- !!! MAX_CORES set to the actual count of the cores causes a page fault on the (MAX_CORES-1) core after entering scheduler loop now, it used to work fine

- validating user memory areas
    maybe at the granularity of the 64kB slabs of pages that i give each malloc

- now that we can redirect logs, write automated tests for cretain parts of kernel and for userspace programs
- create test suite userspace programs that will run and test stuff

- make sure loaded program sections get page-aligned and have actual proper protection flags

- a way to prealloc space for userspace process, let the process specify a requested preallocated size that sys_allocate and the user arena global allocator will know about

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



name vault:
- Argo
- Arkad / Arcad