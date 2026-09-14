use gpui::SharedUri;
use regex::Regex;

/// Extracts media URLs from a string and returns both the extracted URLs
/// and the string with media URLs removed
pub struct MediaExtractor {
    image_regex: Regex,
    video_regex: Regex,
}

impl MediaExtractor {
    /// Creates a new MediaExtractor with compiled regex patterns
    pub fn new() -> Self {
        MediaExtractor {
            // Match common image extensions
            image_regex: Regex::new(
                r#"(?i)\bhttps?://[^\s<>"']+\.(?:jpg|jpeg|png|gif|bmp|webp|svg|ico)(?:\?[^\s<>"']*)?\b"#,
            ).unwrap(),
            // Match common video extensions
            video_regex: Regex::new(
                r#"(?i)\bhttps?://[^\s<>"']+\.(?:mp4|mov|avi|mkv|webm|flv|wmv|m4v|3gp)(?:\?[^\s<>"']*)?\b"#,
            ).unwrap(),
        }
    }

    /// Extracts all media URLs from a string
    pub fn extract_media_urls(&self, text: &str) -> Vec<SharedUri> {
        let mut urls = Vec::new();

        // Extract image URLs
        for capture in self.image_regex.find_iter(text) {
            urls.push(capture.as_str().to_string().into());
        }

        // Extract video URLs
        // for capture in self.video_regex.find_iter(text) {
        //     urls.push(capture.as_str().to_string().into());
        // }

        urls
    }

    /// Removes all media URLs from a string and returns the cleaned text
    pub fn remove_media_urls(&self, text: &str) -> String {
        let mut result = text.to_string();

        // Remove image URLs
        result = self.image_regex.replace_all(&result, "").to_string();

        // Remove video URLs
        // result = self.video_regex.replace_all(&result, "").to_string();

        // Whitespace is part of the message: Markdown uses newlines, indentation,
        // and trailing spaces for block structure and hard breaks.
        result
    }

    /// Extracts media URLs and removes them from the string, returning both
    pub fn extract_and_remove(&self, text: &str) -> (Vec<SharedUri>, String) {
        let urls = self.extract_media_urls(text);
        let cleaned_text = self.remove_media_urls(text);
        (urls, cleaned_text)
    }

    /// Validates if a URL is a valid media URL
    pub fn is_media_url(&self, url: &str) -> bool {
        self.image_regex.is_match(url) || self.video_regex.is_match(url)
    }

    /// Categorizes extracted URLs into images and videos
    pub fn categorize_urls(&self, urls: &[SharedUri]) -> (Vec<SharedUri>, Vec<SharedUri>) {
        let mut images = Vec::new();
        let mut videos = Vec::new();

        for url in urls {
            if self.image_regex.is_match(url) {
                images.push(url.clone());
            } else if self.video_regex.is_match(url) {
                videos.push(url.clone());
            }
        }

        (images, videos)
    }
}

impl Default for MediaExtractor {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience function for one-time extraction and removal
pub fn extract_and_remove_media_urls(text: &str) -> (Vec<SharedUri>, String) {
    let extractor = MediaExtractor::new();
    extractor.extract_and_remove(text)
}

/// Convenience function for just extracting media URLs
pub fn extract_media_urls(text: &str) -> Vec<SharedUri> {
    let extractor = MediaExtractor::new();
    extractor.extract_media_urls(text)
}

/// Convenience function for just removing media URLs
pub fn remove_media_urls(text: &str) -> String {
    let extractor = MediaExtractor::new();
    extractor.remove_media_urls(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_markdown_whitespace_without_media() {
        let source = "Intro  \nnext\n\n- first\n  - nested\n\n```\n  indented\n```\n";
        let (media, text) = extract_and_remove_media_urls(source);
        assert!(media.is_empty());
        assert_eq!(text, source);
    }

    #[test]
    fn removing_media_preserves_surrounding_lines_and_indentation() {
        let source = "Intro\nhttps://example.com/image.png\n\n1. first\n   - nested\n";
        let (media, text) = extract_and_remove_media_urls(source);
        assert_eq!(media.len(), 1);
        assert_eq!(text, "Intro\n\n\n1. first\n   - nested\n");
    }
}
