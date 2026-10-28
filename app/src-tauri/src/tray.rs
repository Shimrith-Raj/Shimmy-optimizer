//! The tray icon: a ring that fills clockwise from the top with RAM use, coloured by level. Drawn with 4x4
//! supersampling so it stays smooth at 16-32 px.

pub const SIZE: u32 = 32;

fn level_rgb(pct: u64) -> [u8; 3] {
  match pct {
    85.. => [255, 77, 61],
    70.. => [255, 176, 32],
    _ => [198, 244, 50],
  }
}

pub fn ring_rgba(pct: u64) -> Vec<u8> {
  let size = SIZE as f32;
  let (c, outer, inner) = (size / 2.0, size / 2.0 - 1.5, size / 2.0 - 7.5);
  let fill = pct.min(100) as f32 / 100.0;
  let on = level_rgb(pct);
  let mut px = vec![0u8; (SIZE * SIZE * 4) as usize];
  for y in 0..SIZE {
    for x in 0..SIZE {
      let (mut lit, mut track) = (0u32, 0u32);
      for sy in 0..4 {
        for sx in 0..4 {
          let dx = x as f32 + (sx as f32 + 0.5) / 4.0 - c;
          let dy = y as f32 + (sy as f32 + 0.5) / 4.0 - c;
          let r = (dx * dx + dy * dy).sqrt();
          if r < inner || r > outer {
            continue;
          }
          // Angle from 12 o'clock, clockwise, as a fraction of a turn.
          let turn = (dx.atan2(-dy) / std::f32::consts::TAU).rem_euclid(1.0);
          if turn <= fill { lit += 1 } else { track += 1 }
        }
      }
      let i = ((y * SIZE + x) * 4) as usize;
      let a_lit = lit as f32 / 16.0;
      let a_track = track as f32 / 16.0 * 0.35;
      let a = a_lit + a_track;
      if a > 0.0 {
        for k in 0..3 {
          px[i + k] = ((on[k] as f32 * a_lit + 238.0 * a_track) / a).round() as u8;
        }
        px[i + 3] = (a.min(1.0) * 255.0).round() as u8;
      }
    }
  }
  px
}

#[cfg(test)]
mod tests {
  #[test]
  fn ring_fills_with_pct() {
    let lit = |pct| super::ring_rgba(pct).chunks(4).filter(|p| p[3] > 200).count();
    assert!(lit(80) > lit(20));
    assert_eq!(super::ring_rgba(50).len(), (super::SIZE * super::SIZE * 4) as usize);
  }
}
