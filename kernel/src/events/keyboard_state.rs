use pc_keyboard::KeyCode;

use crate::events::event_buffer::KeyState;

pub const KEY_COUNT: usize = 256;
const BITS_PER_WORD: usize = 64;
const BITSET_WORD_COUNT: usize = KEY_COUNT / BITS_PER_WORD;

impl KeyState {
    pub fn from_raw(raw: u32) -> Self {
        debug_assert!(raw <= KeyState::Released as u32);
        if raw == KeyState::Pressed as u32 {
            KeyState::Pressed
        } else {
            KeyState::Released
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyBitset {
    words: [u64; BITSET_WORD_COUNT],
}

impl KeyBitset {
    pub const fn new() -> Self {
        Self {
            words: [0; BITSET_WORD_COUNT],
        }
    }

    #[inline]
    pub fn test(&self, key: u8) -> bool {
        let index = key as usize;
        (self.words[index / BITS_PER_WORD] & (1u64 << (index % BITS_PER_WORD))) != 0
    }

    #[inline]
    pub fn set(&mut self, key: u8) {
        let index = key as usize;
        self.words[index / BITS_PER_WORD] |= 1u64 << (index % BITS_PER_WORD);
    }

    #[inline]
    pub fn clear(&mut self, key: u8) {
        let index = key as usize;
        self.words[index / BITS_PER_WORD] &= !(1u64 << (index % BITS_PER_WORD));
    }

    #[inline]
    pub fn clear_all(&mut self) {
        self.words = [0; BITSET_WORD_COUNT];
    }
}

#[derive(Debug, Clone, Copy)]
pub struct KeyboardState {
    // keys currently held
    pub key_down: KeyBitset,
    // keys that went down since the last compositor update
    pub key_pressed: KeyBitset,
    // keys that went up since the last compositor update
    pub key_released: KeyBitset,
}

impl KeyboardState {
    pub const fn new() -> Self {
        Self {
            key_down: KeyBitset::new(),
            key_pressed: KeyBitset::new(),
            key_released: KeyBitset::new(),
        }
    }

    #[inline]
    pub fn is_down(&self, key: KeyCode) -> bool {
        self.key_down.test(key as u8)
    }

    #[inline]
    pub fn was_pressed(&self, key: KeyCode) -> bool {
        self.key_pressed.test(key as u8)
    }

    #[inline]
    pub fn was_released(&self, key: KeyCode) -> bool {
        self.key_released.test(key as u8)
    }

    pub fn update(&mut self, key: u8, state: KeyState) {
        match state {
            KeyState::Pressed => {
                if !self.key_down.test(key) {
                    self.key_pressed.set(key);
                }
                self.key_down.set(key);
            }
            KeyState::Released => {
                self.key_down.clear(key);
                self.key_released.set(key);
            }
        }
    }

    pub fn clear_transient(&mut self) {
        self.key_pressed.clear_all();
        self.key_released.clear_all();
    }
}
