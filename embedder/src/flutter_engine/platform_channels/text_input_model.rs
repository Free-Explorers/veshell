#![allow(dead_code)]

use crate::flutter_engine::platform_channels::text_range::TextRange;

fn is_leading_surrogate(code_point: u32) -> bool {
    (code_point & 0xfffffc00) == 0xd800
}

fn is_trailing_surrogate(code_point: u32) -> bool {
    (code_point & 0xfffffc00) == 0xdc00
}

#[derive(Default)]
pub struct TextInputModel {
    text: Vec<u16>,
    selection: TextRange,
    composing_range: TextRange,
    composing: bool,
}

impl TextInputModel {
    pub fn set_text(
        &mut self,
        text: &str,
        selection: TextRange,
        composing_range: TextRange,
    ) -> bool {
        self.text = text.encode_utf16().collect();
        if !self.text_range().contains_range(&selection)
            || !self.text_range().contains_range(&composing_range)
        {
            return false;
        }
        self.selection = selection;
        self.composing_range = composing_range;
        self.composing = !composing_range.collapsed();
        true
    }

    pub fn set_selection(&mut self, selection: TextRange) -> bool {
        if self.composing && !selection.collapsed() {
            return false;
        }
        if !self.editable_range().contains_range(&selection) {
            return false;
        }
        self.selection = selection;
        true
    }

    pub fn set_composing_range(
        &mut self,
        composing_range: TextRange,
        cursor_offset: usize,
    ) -> bool {
        if !self.composing || !self.text_range().contains_range(&composing_range) {
            return false;
        }
        self.composing_range = composing_range;
        self.selection = TextRange::new_position(composing_range.start() + cursor_offset);
        true
    }

    pub fn begin_composing(&mut self) {
        self.composing = true;
        self.composing_range = TextRange::new_position(self.selection.start());
    }

    pub fn update_composing_text_selection(&mut self, text: &[u16], selection: &TextRange) {
        if text.is_empty() && self.composing_range.collapsed() {
            return;
        }
        let range_to_delete = if self.composing_range.collapsed() {
            &self.selection
        } else {
            &self.composing_range
        };
        self.text.splice(
            range_to_delete.start()..range_to_delete.end(),
            text.iter().cloned(),
        );
        self.composing_range
            .set_end(self.composing_range.start() + text.len());
        self.selection = TextRange::new(
            selection.start() + self.composing_range.start(),
            selection.extent() + self.composing_range.start(),
        );
    }

    pub fn update_composing_text(&mut self, text: &[u16]) {
        self.update_composing_text_selection(text, &TextRange::new_position(text.len()));
    }

    pub fn update_composing_text_utf8(&mut self, text: &str) {
        let encoded: Vec<u16> = text.encode_utf16().collect();
        self.update_composing_text(&encoded);
    }

    pub fn commit_composing(&mut self) {
        if self.composing_range.collapsed() {
            return;
        }
        self.composing_range = TextRange::new_position(self.composing_range.end());
        self.selection = self.composing_range;
    }

    pub fn end_composing(&mut self) {
        self.composing = false;
        self.composing_range = TextRange::new_position(0);
    }

    pub fn delete_selected(&mut self) -> bool {
        if self.selection.collapsed() {
            return false;
        }
        let start = self.selection.start();
        self.text
            .splice(start..self.selection.end(), Vec::new().iter().cloned());
        self.selection = TextRange::new_position(start);
        if self.composing {
            self.composing_range = self.selection;
        }
        true
    }

    pub fn add_char_point(&mut self, c: char) {
        if c as u32 <= 0xFFFF {
            self.add_text(&[c as u16])
        } else {
            let to_decompose = c as u32 - 0x10000;
            self.add_text(&[
                ((to_decompose >> 10) + 0xd800) as u16,
                ((to_decompose % 0x400) + 0xdc00) as u16,
            ])
        }
    }

    pub fn add_text(&mut self, text: &[u16]) {
        self.delete_selected();
        if self.composing {
            self.text.splice(
                self.composing_range.start()..self.composing_range.end(),
                text.iter().cloned(),
            );
            self.selection = TextRange::new_position(self.composing_range.start());
            self.composing_range
                .set_end(self.composing_range.start() + text.len());
        }
        let position = self.selection.position();
        self.text.splice(position..position, text.iter().cloned());
        self.selection = TextRange::new_position(position + text.len());
    }

    pub fn backspace(&mut self) -> bool {
        if self.delete_selected() {
            return true;
        }
        let position = self.selection.position();
        if position != self.editable_range().start() {
            let count = if is_trailing_surrogate(self.text[position - 1] as u32) {
                2
            } else {
                1
            };
            self.text
                .splice(position - count..position, Vec::new().iter().cloned());
            self.selection = TextRange::new_position(position - count);
            if self.composing {
                self.composing_range
                    .set_end(self.composing_range.end() - count);
            }
            return true;
        }
        false
    }

    pub fn delete(&mut self) -> bool {
        if self.delete_selected() {
            return true;
        }
        let position = self.selection.position();
        if position < self.editable_range().end() {
            let count = if is_leading_surrogate(self.text[position] as u32) {
                2
            } else {
                1
            };
            self.text
                .splice(position..position + count, Vec::new().iter().cloned());
            if self.composing {
                self.composing_range
                    .set_end(self.composing_range.end() - count);
            }
            return true;
        }
        false
    }

    pub fn delete_surroundings(&mut self, offset_from_cursor: i32, mut count: usize) -> bool {
        let max_pos = self.editable_range().end();
        let mut start = self.selection.extent();
        if offset_from_cursor < 0 {
            for i in 0..(-offset_from_cursor) as usize {
                if start == self.editable_range().start() {
                    count = i;
                    break;
                }
                start -= if is_leading_surrogate(self.text[start - 1] as u32) {
                    2
                } else {
                    1
                };
            }
        } else {
            for _ in 0..offset_from_cursor {
                if start == max_pos {
                    break;
                }
                start += if is_leading_surrogate(self.text[start] as u32) {
                    2
                } else {
                    1
                };
            }
        }

        let mut end = start;
        for _ in 0..count {
            if end == max_pos {
                break;
            }
            end += if is_leading_surrogate(self.text[start] as u32) {
                2
            } else {
                1
            };
        }

        if start == end {
            return false;
        }

        let deleted_length = end - start;
        self.text.splice(start..end, Vec::new().iter().cloned());

        self.selection = TextRange::new_position(if offset_from_cursor <= 0 {
            start
        } else {
            self.selection.start()
        });

        if self.composing {
            self.composing_range
                .set_end(self.composing_range.end() - deleted_length);
        }
        true
    }

    pub fn move_cursor_to_beginning(&mut self) -> bool {
        let min_pos = self.editable_range().start();
        if self.selection.collapsed() && self.selection.position() == min_pos {
            return false;
        }
        self.selection = TextRange::new_position(min_pos);
        true
    }

    pub fn move_cursor_to_end(&mut self) -> bool {
        let max_pos = self.editable_range().end();
        if self.selection.collapsed() && self.selection.position() == max_pos {
            return false;
        }
        self.selection = TextRange::new_position(max_pos);
        true
    }

    pub fn select_to_beginning(&mut self) -> bool {
        let min_pos = self.editable_range().start();
        if self.selection.collapsed() && self.selection.position() == min_pos {
            return false;
        }
        self.selection = TextRange::new(self.selection.base(), min_pos);
        true
    }

    pub fn select_to_end(&mut self) -> bool {
        let max_pos = self.editable_range().end();
        if self.selection.collapsed() && self.selection.position() == max_pos {
            return false;
        }
        self.selection = TextRange::new(self.selection.base(), max_pos);
        true
    }

    pub fn move_cursor_forward(&mut self) -> bool {
        if !self.selection.collapsed() {
            self.selection = TextRange::new_position(self.selection.end());
            return true;
        }
        let position = self.selection.position();
        if position != self.editable_range().end() {
            let count = if is_leading_surrogate(self.text[position] as u32) {
                2
            } else {
                1
            };
            self.selection = TextRange::new_position(position + count);
            return true;
        }
        false
    }

    pub fn move_cursor_back(&mut self) -> bool {
        if !self.selection.collapsed() {
            self.selection = TextRange::new_position(self.selection.start());
            return true;
        }
        let position = self.selection.position();
        if position != self.editable_range().start() {
            let count = if is_trailing_surrogate(self.text[position - 1] as u32) {
                2
            } else {
                1
            };
            self.selection = TextRange::new_position(position - count);
            return true;
        }
        false
    }

    pub fn get_text(&self) -> String {
        String::from_utf16_lossy(&self.text)
    }

    pub fn get_cursor_offset(&self) -> usize {
        let leading_text = &self.text[0..self.selection.extent()];
        String::from_utf16_lossy(leading_text).len()
    }

    pub fn text_range(&self) -> TextRange {
        TextRange::new(0, self.text.len())
    }

    pub fn selection(&self) -> TextRange {
        self.selection
    }

    pub fn composing_range(&self) -> TextRange {
        self.composing_range
    }

    pub fn composing(&self) -> bool {
        self.composing
    }

    pub fn editable_range(&self) -> TextRange {
        if self.composing {
            self.composing_range
        } else {
            self.text_range()
        }
    }
}
