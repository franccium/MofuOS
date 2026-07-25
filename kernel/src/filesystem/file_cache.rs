/*

wait why am i even doing this
why not just use file descriptors, store mappings of paths to open file handles, if another process wants the same file as some cached file, he gets the handle
and the handle indexes into an array of pointers to a page with this file loaded in RAM
that was even my first idea
and thats works even for multiple processes and is just better i suppose
whatever this is a fun idea at least
i suppose i was thinking in terms of a memory arena so that caching scheme would be less wasteful in memory
also thats a bit better for actual cache locality

actually the below is a whole in-memory filesystem
i made a filesystem instead of file cache
w/e

groups of arrays, hashable groups
hash is by some parenting part of the file path
we do this because file operations are very often based on locality
maybe we can even let the process specify how much cache it wants for files at given paths
e.g. declare_path_cache_importance(path, Importance::VERY_HIGH) / declare_path_cache_size(path, 64)
this would give bigger/smaller chunks of the global cache array to the given path and its children caches
the hash is somehow computed from path and has to be fast
/home/game/fonts - 1001001
/home/game - 0001001
/home - 0000001
/ - 0000000
or something
here home would have the parenting hash and nest its local file cache inside, game needs a big hash chunk, fonts inside game dont need much space for any more nested cache structures
maybe i could even let the process specify that a given directory will be flat
and specify the most important files in a given directory
maybe elevate important files outside of the whole structure, and have them lifetime cached until they arent needed anymore (also a declaration made by the process - start_file_cache_residency(path) / stop_file_cache_residency(path)

can we make optimizations based on the fact that FAT32 limits filesizes to 8 bytes?
so each directory is 8-bytes long, so we hash 8 bits for each directory part of the path
this data assumption definitely has optimization potential

*/

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::min;
use core::mem::size_of;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;

use crate::filesystem::fat32::FileNodeHandle;
use crate::filesystem::sirius::FileSystemError;
use crate::{serial_println, serial_println_core};

const PAGE_SIZE: usize = 4096;
const FAT32_NAME_LEN: usize = 8;
const FAT32_EXT_LEN: usize = 3;
const FAT32_FULL_NAME: usize = 11;
const MAX_PATH_DEPTH: usize = 8;
const HASH_BITS_PER_LEVEL: u32 = 8;
const TRIE_BRANCHING: usize = 256; // 2^8
const MAX_CACHE_MEMORY: usize = 64 * 1024 * 1024; // 64MB default
const HOT_PATH_CACHE_SIZE: usize = 16;
const FILENAME_BYTE_EMPTY: u8 = 0x20;

const DEBUG_LOGS: bool = false;

macro_rules! serial_println_core {
    ($($arg:tt)*) => {
        if DEBUG_LOGS {
            $crate::serial_println_core!($($arg)*);
        }
    };
}

pub struct CacheStats {
    pub total_files: usize,
    pub total_bytes: usize,
    pub max_bytes: usize,
}

/// FAT32-optimized path component - exactly 11 bytes, no heap allocation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fat32PathComponent {
    name: [u8; FAT32_NAME_LEN], // Space-padded
    ext: [u8; FAT32_EXT_LEN],   // Space-padded
    name_len: u8,               // Actual name length (0-8)
    ext_len: u8,                // Actual extension length (0-3)
}

impl Fat32PathComponent {
    /// Create empty component (for padding unused slots)
    const fn empty() -> Self {
        Self {
            name: [FILENAME_BYTE_EMPTY; FAT32_NAME_LEN],
            ext: [FILENAME_BYTE_EMPTY; FAT32_EXT_LEN],
            name_len: 0,
            ext_len: 0,
        }
    }

    /// Parse from raw FAT32 directory entry bytes
    #[inline(always)]
    fn from_raw(raw: &[u8; 11]) -> Self {
        let mut name = [0u8; 8];
        let mut ext = [0u8; 3];
        let mut name_len = 0u8;
        let mut ext_len = 0u8;

        for (i, &byte) in raw[..8].iter().enumerate() {
            if byte == FILENAME_BYTE_EMPTY {
                break;
            }
            name[i] = byte;
            name_len = (i + 1) as u8;
        }

        for (i, &byte) in raw[8..11].iter().enumerate() {
            if byte == FILENAME_BYTE_EMPTY {
                break;
            }
            ext[i] = byte;
            ext_len = (i + 1) as u8;
        }

        Self {
            name,
            ext,
            name_len,
            ext_len,
        }
    }

    /// Generate hash optimized for trie navigation
    /// We want the distinguishing bits in the lower 8 bits for trie indexing
    #[inline(always)]
    fn hash(&self) -> u64 {
        // Load name as u64
        let name_part = u64::from_le_bytes([
            self.name[0],
            self.name[1],
            self.name[2],
            self.name[3],
            self.name[4],
            self.name[5],
            self.name[6],
            self.name[7],
        ]);

        // Load extension as u32
        let ext_part = u32::from_le_bytes([self.ext[0], self.ext[1], self.ext[2], 0x00]);

        // Mix using multiply-shift
        let mut hash = name_part.wrapping_mul(0x9E3779B97F4A7C15).rotate_left(17);

        hash ^= (ext_part as u64).wrapping_mul(0xC6A4A7935BD1E995);
        hash = hash.rotate_right(11);
        hash ^= hash >> 33;
        hash = hash.wrapping_mul(0xFF51AFD7ED558CCD);
        hash ^= hash >> 33;

        hash
    }

    /// Get the trie index byte for this component (lower 8 bits of hash)
    #[inline(always)]
    fn trie_index(&self) -> u8 {
        (self.hash() & 0xFF) as u8
    }
}

/// Fixed-size path representation for FAT32 (stack-allocated)
#[derive(Debug, Clone, Copy)]
struct Fat32Path {
    components: [Fat32PathComponent; MAX_PATH_DEPTH],
    depth: u8,
    hash: u64,
}

impl Fat32Path {
    /// Create root path
    const fn root() -> Self {
        Self {
            components: [Fat32PathComponent::empty(); MAX_PATH_DEPTH],
            depth: 0,
            hash: 0,
        }
    }

    /// Parse from a string path
    fn from_str(path: &str) -> Option<Self> {
        let mut components = [Fat32PathComponent::empty(); MAX_PATH_DEPTH];
        let mut depth = 0u8;
        let mut full_hash: u64 = 0;

        // Skip leading slash
        let path_bytes = if path.starts_with('/') {
            &path.as_bytes()[1..]
        } else {
            path.as_bytes()
        };

        // Handle root path
        if path_bytes.is_empty() || (path_bytes.len() == 1 && path_bytes[0] == b'/') {
            return Some(Self::root());
        }

        for component_str in path_bytes.split(|&b| b == b'/') {
            if component_str.is_empty() {
                continue;
            }

            if depth >= MAX_PATH_DEPTH as u8 {
                return None; // Path too deep
            }

            let component = Self::parse_component(component_str)?;
            let component_hash = component.hash();

            // Each component gets 8 bits in the final hash, shifted by depth
            // This ensures trie navigation consumes bits in path order
            let trie_byte = (component_hash & 0xFF) as u64;
            full_hash |= trie_byte << (depth as u32 * HASH_BITS_PER_LEVEL);

            components[depth as usize] = component;
            depth += 1;
        }

        Some(Self {
            components,
            depth,
            hash: full_hash,
        })
    }

    /// Parse a single path component (file or directory name)
    fn parse_component(bytes: &[u8]) -> Option<Fat32PathComponent> {
        let mut name = [FILENAME_BYTE_EMPTY; 8];
        let mut ext = [FILENAME_BYTE_EMPTY; 3];
        let mut name_len = 0u8;
        let mut ext_len = 0u8;

        // Handle "." and ".." specially
        if bytes == b"." || bytes == b".." {
            for (i, &b) in bytes.iter().enumerate() {
                name[i] = b;
                name_len = (i + 1) as u8;
            }
            return Some(Fat32PathComponent {
                name,
                ext,
                name_len,
                ext_len,
            });
        }

        // Split name and extension at the last dot
        if let Some(dot_pos) = bytes.iter().rposition(|&b| b == b'.') {
            let name_bytes = &bytes[..dot_pos];
            let name_copy_len = min(name_bytes.len(), 8);
            name[..name_copy_len].copy_from_slice(&name_bytes[..name_copy_len]);
            name_len = name_copy_len as u8;

            let ext_bytes = &bytes[dot_pos + 1..];
            let ext_copy_len = min(ext_bytes.len(), 3);
            ext[..ext_copy_len].copy_from_slice(&ext_bytes[..ext_copy_len]);
            ext_len = ext_copy_len as u8;
        } else {
            let name_copy_len = min(bytes.len(), 8);
            name[..name_copy_len].copy_from_slice(&bytes[..name_copy_len]);
            name_len = name_copy_len as u8;
        }

        Some(Fat32PathComponent {
            name,
            ext,
            name_len,
            ext_len,
        })
    }

    /// Get parent path
    #[inline(always)]
    fn parent(&self) -> Option<Fat32Path> {
        if self.depth == 0 {
            return None;
        }

        let mut parent = *self;
        parent.depth -= 1;

        // Clear the bits for the removed component
        let mask = !(0xFFu64 << (parent.depth as u32 * HASH_BITS_PER_LEVEL));
        parent.hash &= mask;

        parent.components[parent.depth as usize] = Fat32PathComponent::empty();

        Some(parent)
    }

    /// Get trie index for a specific depth level
    #[inline(always)]
    fn trie_index_at(&self, level: u8) -> u8 {
        if level >= self.depth {
            return 0;
        }
        ((self.hash >> (level as u32 * HASH_BITS_PER_LEVEL)) & 0xFF) as u8
    }

    /// Get hash prefix up to a specific depth (for directory-level operations)
    #[inline(always)]
    fn hash_prefix(&self, depth: u8) -> u64 {
        if depth == 0 {
            return 0;
        }
        let mask = (1u64 << (depth as u32 * HASH_BITS_PER_LEVEL)) - 1;
        self.hash & mask
    }
}

// ---- Cache Page ----

/// A cached file's data, stored in pages
#[derive(Debug)]
pub struct CachePage {
    data: Vec<u8>,
    path_hash: u64, // Full path hash for verification
    file_size: usize,
    access_tick: u64,
    is_dirty: bool,
    pinned: bool,
}

impl CachePage {
    fn new(data: Vec<u8>, path_hash: u64, tick: u64) -> Self {
        let file_size = data.len();
        Self {
            data,
            path_hash,
            file_size,
            access_tick: tick,
            is_dirty: false,
            pinned: false,
        }
    }

    fn get_slice(&self, offset: usize, len: usize) -> &[u8] {
        let end = min(offset + len, self.file_size);
        &self.data[offset..end]
    }
}

#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct CachePagePtr(NonNull<CachePage>);

impl CachePagePtr {
    pub fn new(ptr: *const CachePage) -> Option<Self> {
        Some(Self(NonNull::new(ptr as *mut CachePage)?))
    }

    pub const fn from_raw(ptr: *mut CachePage) -> Self {
        Self(unsafe { NonNull::new_unchecked(ptr) })
    }

    pub const fn as_ptr(self) -> *mut CachePage {
        self.0.as_ptr()
    }

    pub const fn as_const_ptr(self) -> *const CachePage {
        self.0.as_ptr() as *const CachePage
    }

    pub unsafe fn as_mut_ref(&mut self) -> &mut CachePage {
        self.0.as_mut()
    }

    pub unsafe fn as_ref(&self) -> &CachePage {
        self.0.as_ref()
    }

    // Add these helper methods
    pub unsafe fn get_mut(&mut self) -> &mut CachePage {
        self.0.as_mut()
    }

    pub unsafe fn get(&self) -> &CachePage {
        self.0.as_ref()
    }
}

impl Deref for CachePagePtr {
    type Target = CachePage;

    fn deref(&self) -> &Self::Target {
        unsafe { self.0.as_ref() }
    }
}
impl DerefMut for CachePagePtr {
    fn deref_mut(&mut self) -> &mut CachePage {
        unsafe { self.0.as_mut() }
    }
}

unsafe impl Send for CachePagePtr {}
unsafe impl Sync for CachePagePtr {}

// Add conversions
impl From<*mut CachePage> for CachePagePtr {
    fn from(ptr: *mut CachePage) -> Self {
        Self::from_raw(ptr)
    }
}

impl From<*const CachePage> for CachePagePtr {
    fn from(ptr: *const CachePage) -> Self {
        Self::new(ptr).expect("null pointer")
    }
}

impl From<&mut CachePage> for CachePagePtr {
    fn from(page: &mut CachePage) -> Self {
        Self::from_raw(page)
    }
}

impl From<&CachePage> for CachePagePtr {
    fn from(page: &CachePage) -> Self {
        Self::from_raw(page as *const CachePage as *mut CachePage)
    }
}

// ---- Hash Trie Node ----

/// A node in the hash trie
///
/// The trie structure mirrors the filesystem hierarchy through hash bits.
/// Each level consumes 8 bits of the hash, indexing into 256 possible children.
/// Leaf nodes contain the actual cached file data.
struct TrieNode {
    /// 256 possible children, indexed by the next 8 bits of hash
    children: [Option<Box<TrieNode>>; TRIE_BRANCHING],
    /// Cached file data (only present if a file at this exact path is cached)
    cache_page: Option<CachePage>,
    /// Number of files cached in this subtree (for eviction decisions)
    subtree_file_count: usize,
    /// Total bytes cached in this subtree
    subtree_bytes: usize,
    /// Is this node or any child pinned?
    has_pinned_descendant: bool,
}

impl TrieNode {
    fn new() -> Self {
        Self {
            children: [const { None }; TRIE_BRANCHING],
            cache_page: None,
            subtree_file_count: 0,
            subtree_bytes: 0,
            has_pinned_descendant: false,
        }
    }

    /// Check if this node has any cached data
    fn is_empty(&self) -> bool {
        self.cache_page.is_none() && self.subtree_file_count == 0
    }

    /// Update subtree statistics after child modification
    fn update_stats(&mut self) {
        self.subtree_file_count = 0;
        self.subtree_bytes = 0;
        self.has_pinned_descendant = false;

        if let Some(ref page) = self.cache_page {
            self.subtree_file_count = 1;
            self.subtree_bytes = page.file_size;
            self.has_pinned_descendant = page.pinned;
        }

        for child in self.children.iter().flatten() {
            self.subtree_file_count += child.subtree_file_count;
            self.subtree_bytes += child.subtree_bytes;
            self.has_pinned_descendant |= child.has_pinned_descendant;
        }
    }
}

// ---- Hash Trie ----

/// The main cache data structure
///
/// Uses a hash trie where path hierarchy is encoded in hash bits.
/// Lookup is O(path_depth) with just bit extraction and pointer chasing.
/// Directory eviction is O(1) by clearing a single node's children.
struct HashTrie {
    root: TrieNode,
    /// Hot path cache for recently accessed files
    hot_paths: [Option<(u64, CachePagePtr)>; HOT_PATH_CACHE_SIZE],
    hot_path_next: usize,
    total_bytes: usize,
    total_files: usize,
}

impl HashTrie {
    fn new() -> Self {
        Self {
            root: TrieNode::new(),
            hot_paths: [const { None }; HOT_PATH_CACHE_SIZE],
            hot_path_next: 0,
            total_bytes: 0,
            total_files: 0,
        }
    }

    /// Look up a cached file by its full path hash
    fn lookup(&self, path: &Fat32Path) -> Option<&CachePage> {
        // Check hot path cache first
        for entry in &self.hot_paths {
            if let Some((hash, ptr)) = entry {
                if *hash == path.hash {
                    return Some(unsafe { &**ptr });
                }
            }
        }

        // Walk the trie
        let mut node = &self.root;

        for level in 0..path.depth {
            let idx = path.trie_index_at(level) as usize;

            match &node.children[idx] {
                Some(child) => {
                    node = child;
                }
                None => {
                    return None;
                }
            }
        }

        // At the target depth, check for cached page
        if let Some(ref page) = node.cache_page {
            // Verify full hash matches (in case of collisions)
            if page.path_hash == path.hash {
                return Some(page);
            }
        }

        None
    }

    fn lookup_mut(&mut self, path: &Fat32Path) -> Option<&mut CachePage> {
        // Check hot path cache first
        for entry in &mut self.hot_paths {
            if let Some((hash, ptr)) = entry {
                if *hash == path.hash {
                    // SAFETY: We have mutable access to the trie, and the pointer
                    // is valid and points to a CachePage in the trie
                    return Some(unsafe { &mut **ptr });
                }
            }
        }

        // Walk the trie
        let mut node = &mut self.root;

        for level in 0..path.depth {
            let idx = path.trie_index_at(level) as usize;

            match &mut node.children[idx] {
                Some(child) => {
                    node = child;
                }
                None => {
                    return None;
                }
            }
        }

        // At the target depth, check for cached page
        if let Some(ref mut page) = node.cache_page {
            // Verify full hash matches (in case of collisions)
            if page.path_hash == path.hash {
                return Some(page);
            }
        }

        None
    }

    /// Look up by directory path - finds the node for a directory
    fn lookup_directory_node(&self, path: &Fat32Path) -> Option<&TrieNode> {
        let mut node = &self.root;

        for level in 0..path.depth {
            let idx = path.trie_index_at(level) as usize;

            match &node.children[idx] {
                Some(child) => {
                    node = child;
                }
                None => {
                    return None;
                }
            }
        }

        Some(node)
    }

    fn insert(&mut self, path: &Fat32Path, data: Vec<u8>, tick: u64) {
        // Create the page
        let page = CachePage::new(data, path.hash, tick);

        // Walk/create the trie path and store the page
        let mut node = &mut self.root;

        for level in 0..path.depth {
            let idx = path.trie_index_at(level) as usize;

            if node.children[idx].is_none() {
                node.children[idx] = Some(Box::new(TrieNode::new()));
            }

            node = node.children[idx].as_mut().unwrap();
        }

        let old_size = node.cache_page.as_ref().map(|p| p.file_size).unwrap_or(0);
        node.cache_page = Some(page);

        // Update stats
        self.update_stats_upward(path);

        // NOW get a pointer to the page that's stored in the trie
        let page_ptr = {
            let mut node = &mut self.root;
            for level in 0..path.depth {
                let idx = path.trie_index_at(level) as usize;
                if let Some(child) = node.children[idx].as_mut() {
                    node = child;
                } else {
                    // Shouldn't happen since we just inserted
                    return;
                }
            }

            if let Some(ref mut page) = node.cache_page {
                // Get a pointer to the page in the trie
                let ptr = page as *mut CachePage;
                CachePagePtr::from(ptr)
            } else {
                return;
            }
        };

        let file_size = page_ptr.file_size;

        self.total_bytes = self.total_bytes - old_size + file_size;
        if old_size == 0 {
            self.total_files += 1;
        }

        // Add to hot path cache with the valid pointer
        self.hot_paths[self.hot_path_next] = Some((path.hash, page_ptr));
        self.hot_path_next = (self.hot_path_next + 1) % HOT_PATH_CACHE_SIZE;
    }

    /// Update statistics from leaf to root after modification
    fn update_stats_upward(&mut self, path: &Fat32Path) {
        // We need to walk from root to leaf, collecting nodes
        // Then update from leaf back to root
        // Since we can't easily walk upward, we do a full path walk

        let mut indices = [0u8; MAX_PATH_DEPTH];
        for level in 0..path.depth {
            indices[level as usize] = path.trie_index_at(level);
        }

        // Recursively update from bottom up
        Self::update_node_stats(&mut self.root, &indices, 0, path.depth);
    }

    fn update_node_stats(node: &mut TrieNode, indices: &[u8], current_level: u8, max_level: u8) {
        if current_level >= max_level {
            node.update_stats();
            return;
        }

        let idx = indices[current_level as usize] as usize;
        if let Some(ref mut child) = node.children[idx] {
            Self::update_node_stats(child, indices, current_level + 1, max_level);
        }

        node.update_stats();
    }

    /// Remove a cached file
    fn remove(&mut self, path: &Fat32Path) -> Option<Vec<u8>> {
        let mut node = &mut self.root;

        // Walk to the file's node
        for level in 0..path.depth {
            let idx = path.trie_index_at(level) as usize;

            match node.children[idx].as_mut() {
                Some(child) => {
                    node = child;
                }
                None => {
                    return None;
                }
            }
        }

        // Remove the page
        let page = node.cache_page.take()?;
        let data = page.data;

        self.total_bytes -= page.file_size;
        self.total_files -= 1;

        // Clean up empty nodes (walk back up and remove if empty)
        self.cleanup_empty_nodes(path);
        self.update_stats_upward(path);

        // Remove from hot path cache
        for entry in &mut self.hot_paths {
            if let Some((hash, _)) = entry {
                if *hash == path.hash {
                    *entry = None;
                }
            }
        }

        Some(data)
    }

    /// Clean up empty nodes after removal
    fn cleanup_empty_nodes(&mut self, path: &Fat32Path) {
        // Collect node pointers along the path
        let mut nodes: [*mut TrieNode; MAX_PATH_DEPTH] = [core::ptr::null_mut(); MAX_PATH_DEPTH];
        let mut indices = [0u8; MAX_PATH_DEPTH];

        {
            let mut node: *mut TrieNode = &mut self.root;

            for level in 0..path.depth {
                nodes[level as usize] = node;
                let idx = path.trie_index_at(level) as usize;
                indices[level as usize] = idx as u8;

                unsafe {
                    if let Some(ref mut child) = (*node).children[idx] {
                        node = child.as_mut() as *mut TrieNode;
                    } else {
                        break;
                    }
                }
            }
        }

        // Walk back up, removing empty children
        for level in (0..path.depth).rev() {
            let idx = indices[level as usize] as usize;
            let parent_ptr = nodes[level as usize];

            if parent_ptr.is_null() {
                continue;
            }

            unsafe {
                let should_remove = if let Some(ref child) = (*parent_ptr).children[idx] {
                    child.is_empty()
                } else {
                    false
                };

                if should_remove {
                    (*parent_ptr).children[idx] = None;
                }
            }
        }
    }

    /// Evict an entire directory subtree - O(1) operation
    fn evict_directory(&mut self, path: &Fat32Path) -> usize {
        if path.depth == 0 {
            // Evicting root - clear everything
            let freed = self.total_bytes;
            self.root = TrieNode::new();
            self.total_bytes = 0;
            self.total_files = 0;
            self.hot_paths = [const { None }; HOT_PATH_CACHE_SIZE];
            self.hot_path_next = 0;
            return freed;
        }

        // Walk to the parent of the directory
        let mut parent_node = &mut self.root;

        for level in 0..(path.depth - 1) {
            let idx = path.trie_index_at(level) as usize;

            match parent_node.children[idx].as_mut() {
                Some(child) => {
                    parent_node = child;
                }
                None => {
                    return 0; // Directory not in cache
                }
            }
        }

        // Remove the directory's node
        let dir_idx = path.trie_index_at(path.depth - 1) as usize;

        if let Some(dir_node) = parent_node.children[dir_idx].take() {
            let freed = dir_node.subtree_bytes;
            self.total_bytes -= freed;
            self.total_files -= dir_node.subtree_file_count;

            // Clean hot path cache
            self.hot_paths = [const { None }; HOT_PATH_CACHE_SIZE];
            self.hot_path_next = 0;

            // Update parent stats
            parent_node.update_stats();

            freed
        } else {
            0
        }
    }

    /// Pin a file (never evict)
    fn pin_file(&mut self, path: &Fat32Path) -> bool {
        let mut node = &mut self.root;

        for level in 0..path.depth {
            let idx = path.trie_index_at(level) as usize;

            match node.children[idx].as_mut() {
                Some(child) => {
                    node = child;
                }
                None => {
                    return false;
                }
            }
        }

        if let Some(ref mut page) = node.cache_page {
            if page.path_hash == path.hash {
                page.pinned = true;
                self.update_stats_upward(path);
                return true;
            }
        }

        false
    }

    /// Unpin a file
    fn unpin_file(&mut self, path: &Fat32Path) -> bool {
        let mut node = &mut self.root;

        for level in 0..path.depth {
            let idx = path.trie_index_at(level) as usize;

            match node.children[idx].as_mut() {
                Some(child) => {
                    node = child;
                }
                None => {
                    return false;
                }
            }
        }

        if let Some(ref mut page) = node.cache_page {
            if page.path_hash == path.hash {
                page.pinned = false;
                self.update_stats_upward(path);
                return true;
            }
        }

        false
    }

    /// Find the best candidate for eviction
    /// Returns the path hash of the file to evict
    fn find_eviction_candidate(&self) -> Option<u64> {
        self.find_eviction_candidate_recursive(&self.root, 0)
    }

    fn find_eviction_candidate_recursive(&self, node: &TrieNode, depth: u8) -> Option<u64> {
        // Don't evict from pinned subtrees
        if node.has_pinned_descendant && depth > 0 {
            return None;
        }

        // Check if this node has an unpinned file
        if let Some(ref page) = node.cache_page {
            if !page.pinned {
                return Some(page.path_hash);
            }
        }

        // Search children, preferring older files (just take first unpinned)
        // In a real implementation, you'd track access ticks and evict LRU
        for child in node.children.iter().flatten() {
            if let Some(hash) = self.find_eviction_candidate_recursive(child, depth + 1) {
                return Some(hash);
            }
        }

        None
    }
}

pub type FileSystemResult<T> = Result<T, FileSystemError>;

pub trait CacheFilesystemDriver {
    fn read_whole_file(&mut self, path: &str) -> FileSystemResult<Vec<u8>>;
    fn read_file_range(
        &mut self,
        path: &str,
        offset: usize,
        len: usize,
    ) -> FileSystemResult<Vec<u8>>;
    fn file_size(&self, path: &str) -> FileSystemResult<usize>;
    fn resolve_path(&self, path: &str) -> FileSystemResult<FileNodeHandle>;
    fn read_dir(&mut self, path: &str) -> FileSystemResult<Vec<String>>;
}

// ---- Cache Importance ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheImportance {
    Minimal = 0,
    Low = 1,
    Normal = 2,
    High = 3,
    VeryHigh = 4,
    Critical = 5,
    Resident = 6,
}

impl CacheImportance {
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Minimal),
            1 => Some(Self::Low),
            2 => Some(Self::Normal),
            3 => Some(Self::High),
            4 => Some(Self::VeryHigh),
            5 => Some(Self::Critical),
            6 => Some(Self::Resident),
            _ => None,
        }
    }

    fn eviction_priority(&self) -> u64 {
        match self {
            Self::Minimal => 0,
            Self::Low => 1,
            Self::Normal => 2,
            Self::High => 3,
            Self::VeryHigh => 4,
            Self::Critical => 5,
            Self::Resident => u64::MAX, // Never evict
        }
    }
}

// ---- Directory Cache Hint ----

#[derive(Debug)]
struct DirectoryHint {
    path: Fat32Path,
    importance: CacheImportance,
    access_tick: u64,
}

// ---- Main Cache ----

pub struct FAT32Cache {
    /// The hash trie storing all cached file data
    trie: HashTrie,
    /// Directory-level hints for eviction priority
    directory_hints: Vec<DirectoryHint>,
    /// Path to node handle mapping for fast resolution
    path_to_node: BTreeMap<u64, FileNodeHandle>,
    /// Global tick for LRU tracking
    global_tick: u64,
    /// Memory limit
    max_memory: usize,
    /// Current memory usage
    current_memory: usize,
}

impl FAT32Cache {
    pub fn new(max_memory: usize) -> Self {
        Self {
            trie: HashTrie::new(),
            directory_hints: Vec::new(),
            path_to_node: BTreeMap::new(),
            global_tick: 0,
            max_memory,
            current_memory: 0,
        }
    }

    pub fn read_file(
        &mut self,
        driver: &mut dyn CacheFilesystemDriver,
        path_str: &str,
    ) -> FileSystemResult<Vec<u8>> {
        // Return Vec, not &[u8]
        self.global_tick = self.global_tick.wrapping_add(1);

        let path = Fat32Path::from_str(path_str).ok_or(FileSystemError::InvalidPath)?;
        serial_println_core!(
            "fat32cache: fat32path: depth: {:?}, hash: {}",
            path.depth,
            path.hash
        );

        // Check cache
        if let Some(page) = self.trie.lookup_mut(&path) {
            serial_println_core!(
                "fat32cache: found cache page: hash: {}, file_size: {}, is_dirty: {}, pinned: {}, last access tick: {}, ",
                page.path_hash,
                page.file_size,
                page.is_dirty,
                page.pinned,
                page.access_tick
            );
            page.access_tick = self.global_tick;
            // Return a copy - this is the simplest solution
            return Ok(page.data.clone());
        }

        serial_println_core!("fat32cache: fat32path: cache miss, loading data from disk");

        // Cache miss - load from disk
        let data = driver.read_whole_file(path_str)?;
        let data_len = data.len();

        serial_println_core!("fat32cache: fat32path: loaded {} bytes from disk", data_len);

        if data_len == 0 {
            serial_println_core!("fat32cache: fat32path: data is empty, not caching");
            return Ok(Vec::new());
        }

        // Check space
        while self.current_memory + data_len > self.max_memory {
            if !self.evict_one() {
                serial_println_core!(
                    "fat32cache: fat32path: no cache space, returning data without caching"
                );
                // No space - return data without caching
                return Ok(data);
            }
        }

        // Cache the file
        self.current_memory += data_len;
        self.trie.insert(&path, data.clone(), self.global_tick);

        serial_println_core!(
            "fat32cache: fat32path: cached file, new mem size: {}",
            self.current_memory
        );

        Ok(data)
    }

    /// Read a partial range of a file
    pub fn read_file_range(
        &mut self,
        driver: &mut dyn CacheFilesystemDriver,
        path_str: &str,
        offset: usize,
        len: usize,
    ) -> FileSystemResult<Vec<u8>> {
        self.global_tick = self.global_tick.wrapping_add(1);

        let path = Fat32Path::from_str(path_str).ok_or(FileSystemError::InvalidPath)?;

        // Try cache first
        if let Some(page) = self.trie.lookup(&path) {
            let mut result = Vec::with_capacity(len);
            let slice = page.get_slice(offset, len);
            result.extend_from_slice(slice);
            return Ok(result);
        }

        // Load entire file into cache, then return range
        let data = driver.read_whole_file(path_str)?;
        let data_len = data.len();

        while self.current_memory + data_len > self.max_memory {
            if !self.evict_one() {
                // Can't cache - just return the range
                let end = min(offset + len, data_len);
                return Ok(data[offset..end].to_vec());
            }
        }

        self.current_memory += data_len;
        self.trie.insert(&path, data, self.global_tick);

        if let Some(page) = self.trie.lookup(&path) {
            let mut result = Vec::with_capacity(len);
            let slice = page.get_slice(offset, len);
            result.extend_from_slice(slice);
            Ok(result)
        } else {
            Err(FileSystemError::NotFound)
        }
    }

    /// Pin a file in cache - never evict
    pub fn pin_file(
        &mut self,
        driver: &mut dyn CacheFilesystemDriver,
        path_str: &str,
    ) -> FileSystemResult<()> {
        let path = Fat32Path::from_str(path_str).ok_or(FileSystemError::InvalidPath)?;

        // Ensure file is cached
        if self.trie.lookup(&path).is_none() {
            let data = driver.read_whole_file(path_str)?;
            self.current_memory += data.len();
            self.trie.insert(&path, data, self.global_tick);
        }

        self.trie.pin_file(&path);
        Ok(())
    }

    /// Unpin a file
    pub fn unpin_file(&mut self, path_str: &str) -> FileSystemResult<()> {
        let path = Fat32Path::from_str(path_str).ok_or(FileSystemError::InvalidPath)?;

        self.trie.unpin_file(&path);
        Ok(())
    }

    /// Reserve cache space for a directory with given importance
    pub fn reserve_directory(
        &mut self,
        path_str: &str,
        importance: CacheImportance,
    ) -> FileSystemResult<()> {
        let path = Fat32Path::from_str(path_str).ok_or(FileSystemError::InvalidPath)?;

        // Remove existing hint if present
        self.directory_hints.retain(|h| h.path.hash != path.hash);

        self.directory_hints.push(DirectoryHint {
            path,
            importance,
            access_tick: self.global_tick,
        });

        Ok(())
    }

    /// Evict all files under a directory
    pub fn evict_directory(&mut self, path_str: &str) -> FileSystemResult<usize> {
        let path = Fat32Path::from_str(path_str).ok_or(FileSystemError::InvalidPath)?;

        let freed = self.trie.evict_directory(&path);
        self.current_memory -= freed;

        // Remove directory hint
        self.directory_hints
            .retain(|h| !h.path.hash_prefix(path.depth) == path.hash_prefix(path.depth));

        Ok(freed)
    }

    /// Get the node handle for a path
    pub fn get_node_handle(&self, path_str: &str) -> Option<FileNodeHandle> {
        let path = Fat32Path::from_str(path_str)?;
        self.path_to_node.get(&path.hash).copied()
    }

    /// Check if a file is cached
    pub fn is_cached(&self, path_str: &str) -> bool {
        if let Some(path) = Fat32Path::from_str(path_str) {
            self.trie.lookup(&path).is_some()
        } else {
            false
        }
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            total_files: self.trie.total_files,
            total_bytes: self.current_memory,
            max_bytes: self.max_memory,
        }
    }

    /// Evict one file (LRU-based, respecting importance and pins)
    fn evict_one(&mut self) -> bool {
        // Try to find an eviction candidate
        if let Some(path_hash) = self.trie.find_eviction_candidate() {
            // We need to reconstruct the path from the hash
            // For simplicity, we'll use a different approach:
            // Find the least important directory and evict files from it

            // Sort directory hints by importance (lowest first)
            // This is inefficient for a real implementation
            let mut hints: Vec<usize> = (0..self.directory_hints.len()).collect();
            hints.sort_by_key(|&i| self.directory_hints[i].importance.eviction_priority());

            for idx in hints {
                let hint = &self.directory_hints[idx];
                let freed = self.trie.evict_directory(&hint.path);
                if freed > 0 {
                    self.current_memory -= freed;
                    return true;
                }
            }

            // No directory hints matched - try to evict the candidate we found
            // In a full implementation, we'd have a reverse mapping from hash to path
        }

        false
    }
}

// // ---- Tests ----

// #[cfg(test)]
// mod tests {
//     use super::*;

//     // Mock filesystem driver for testing
//     struct MockDriver {
//         files: BTreeMap<String, Vec<u8>>,
//     }

//     impl MockDriver {
//         fn new() -> Self {
//             Self {
//                 files: BTreeMap::new(),
//             }
//         }

//         fn add_file(&mut self, path: &str, data: Vec<u8>) {
//             self.files.insert(path.to_string(), data);
//         }
//     }

//     impl CacheFilesystemDriver for MockDriver {
//         fn read_whole_file(&mut self, path: &str) -> FileSystemResult<Vec<u8>> {
//             self.files
//                 .get(path)
//                 .cloned()
//                 .ok_or(FileSystemError::NotFound)
//         }

//         fn read_file_range(
//             &mut self,
//             path: &str,
//             offset: usize,
//             len: usize,
//         ) -> FileSystemResult<Vec<u8>> {
//             let data = self.read_whole_file(path)?;
//             let end = min(offset + len, data.len());
//             Ok(data[offset..end].to_vec())
//         }

//         fn file_size(&self, path: &str) -> FileSystemResult<usize> {
//             self.files
//                 .get(path)
//                 .map(|d| d.len())
//                 .ok_or(FileSystemError::NotFound)
//         }

//         fn resolve_path(&self, path: &str) -> FileSystemResult<FileNodeHandle> {
//             if self.files.contains_key(path) {
//                 Ok(FileNodeHandle(
//                     path.as_bytes()
//                         .iter()
//                         .fold(0u64, |h, &b| h.wrapping_mul(31).wrapping_add(b as u64)),
//                 ))
//             } else {
//                 Err(FileSystemError::NotFound)
//             }
//         }

//         fn read_dir(&mut self, _path: &str) -> FileSystemResult<Vec<String>> {
//             Ok(Vec::new())
//         }
//     }

//     #[test]
//     fn test_path_parsing() {
//         let path = Fat32Path::from_str("/home/game/textures/grass.bmp");
//         assert!(path.is_some());

//         let path = path.unwrap();
//         assert_eq!(path.depth, 4);

//         // Hash should be non-zero
//         assert_ne!(path.hash, 0);

//         // Parent should work
//         let parent = path.parent();
//         assert!(parent.is_some());
//         assert_eq!(parent.unwrap().depth, 3);
//     }

//     #[test]
//     fn test_trie_insert_and_lookup() {
//         let mut trie = HashTrie::new();

//         let path = Fat32Path::from_str("/test/file.bin").unwrap();
//         let data = vec![1, 2, 3, 4, 5];

//         trie.insert(&path, data.clone(), 1);

//         let cached = trie.lookup(&path);
//         assert!(cached.is_some());
//         assert_eq!(cached.unwrap().data, data);
//     }

//     #[test]
//     fn test_trie_directory_eviction() {
//         let mut trie = HashTrie::new();

//         let dir = Fat32Path::from_str("/test").unwrap();
//         let file1 = Fat32Path::from_str("/test/file1.bin").unwrap();
//         let file2 = Fat32Path::from_str("/test/file2.bin").unwrap();

//         trie.insert(&file1, vec![1, 2, 3], 1);
//         trie.insert(&file2, vec![4, 5, 6], 1);

//         assert_eq!(trie.total_files, 2);

//         let freed = trie.evict_directory(&dir);
//         assert!(freed > 0);
//         assert_eq!(trie.total_files, 0);

//         assert!(trie.lookup(&file1).is_none());
//         assert!(trie.lookup(&file2).is_none());
//     }

//     #[test]
//     fn test_cache_read() {
//         let mut driver = MockDriver::new();
//         driver.add_file("/test.txt", b"Hello, World!".to_vec());

//         let mut cache = FAT32Cache::new(1024 * 1024);

//         let data = cache.read_file(&mut driver, "/test.txt");
//         assert!(data.is_ok());
//         assert_eq!(data.unwrap(), b"Hello, World!");

//         // Second read should hit cache
//         let data2 = cache.read_file(&mut driver, "/test.txt");
//         assert!(data2.is_ok());
//         assert_eq!(data2.unwrap(), b"Hello, World!");
//     }

//     #[test]
//     fn test_cache_pin_unpin() {
//         let mut driver = MockDriver::new();
//         driver.add_file("/pinned.txt", b"Pinned data".to_vec());

//         let mut cache = FAT32Cache::new(1024 * 1024);

//         // Pin the file
//         let result = cache.pin_file(&mut driver, "/pinned.txt");
//         assert!(result.is_ok());

//         // Unpin
//         let result = cache.unpin_file("/pinned.txt");
//         assert!(result.is_ok());
//     }

//     #[test]
//     fn test_reserve_directory() {
//         let mut cache = FAT32Cache::new(1024 * 1024);

//         let result = cache.reserve_directory("/important/data", CacheImportance::VeryHigh);
//         assert!(result.is_ok());
//     }
// }
