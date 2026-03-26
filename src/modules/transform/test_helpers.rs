#[cfg(test)]
pub fn tiny_png_bytes() -> Vec<u8> {
  use base64::{Engine, engine::general_purpose::STANDARD};
  STANDARD.decode(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC"
    ).unwrap()
}

#[cfg(test)]
pub fn tiny_jpeg_bytes() -> Vec<u8> {
  use base64::{Engine, engine::general_purpose::STANDARD};
  STANDARD.decode(
        "/9j/4AAQSkZJRgABAQEASABIAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/2wBDAQkJCQwLDBgNDRgyIRwhMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjL/wAARCAABAAEDASIAAhEBAxEB/8QAFgABAQEAAAAAAAAAAAAAAAAABgUE/8QAIhAAAQMEAgMAAAAAAAAAAAAAAQIDBAUREiExQf/EABQBAQAAAAAAAAAAAAAAAAAAAAD/xAAUEQEAAAAAAAAAAAAAAAAAAAAA/9oADAMBAAIRAxEAPwCn2pRqb2/cFPCdSfXFGpIaMHOVHsfmBmMAAA//2Q=="
    ).unwrap()
}

#[cfg(test)]
pub fn tiny_gif_anim_bytes() -> Vec<u8> {
  use image::codecs::gif::{GifEncoder, Repeat};
  use image::{Delay, Frame, RgbaImage};
  use std::io::Cursor;

  let mut buf = Cursor::new(Vec::new());
  let mut encoder = GifEncoder::new(&mut buf);
  encoder.set_repeat(Repeat::Infinite).unwrap();
  // 3 frames: red, green, blue (1x1 pixels)
  let frames = [[255u8, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]];
  for rgba in frames {
    let img = RgbaImage::from_raw(1, 1, rgba.to_vec()).unwrap();
    let frame = Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(100, 1));
    encoder.encode_frame(frame).unwrap();
  }
  drop(encoder);
  buf.into_inner()
}

#[test]
fn test_tiny_gif_anim_has_three_frames() {
  use image::AnimationDecoder;
  use image::codecs::gif::GifDecoder;
  use std::io::Cursor;

  let bytes = tiny_gif_anim_bytes();
  let decoder = GifDecoder::new(Cursor::new(&bytes)).unwrap();
  let frames = decoder.into_frames().collect_frames().unwrap();
  assert_eq!(frames.len(), 3);
}
