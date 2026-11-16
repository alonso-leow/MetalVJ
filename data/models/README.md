# MediaPipe ONNX Models

This directory contains the ONNX-format MediaPipe models used by metal-sense for real pose/hand/face detection.

## Source

MediaPipe models are provided by Google under the Apache 2.0 license (permissive).

Download from: https://developers.google.com/mediapipe/solutions/vision

## Models Used

### Pose Detection

- **File**: `pose_landmarker_heavy.onnx`
- **Landmarks**: 33 points (full body pose)
- **Input**: RGB image (height, width, 3)
- **Output**: Keypoints with confidence scores
- **Use**: `sensor.yolo` node with `Mode::Eyes` for camera tracking

### Hand Detection

- **File**: `hand_landmarker.onnx`
- **Landmarks**: 21 points per hand (fingers, palm)
- **Input**: RGB image (height, width, 3)
- **Output**: Keypoints for left and right hands
- **Use**: `sensor.yolo` node with `Mode::Hands` for fader control

### Face Detection (Future)

- **File**: `face_landmarker.onnx`
- **Landmarks**: 478 points (face contours)
- **Input**: RGB image (height, width, 3)
- **Output**: Face landmarks with visibility and presence scores
- **Status**: Approved for future implementation (O.9b)

## O.9 Measurements (Pending)

The following benchmarks are still pending and will be taken once models are ready:

- Frame-rate on Linux desktop (with wgpu GPU backend)
- Frame-rate on Raspberry Pi (with wgpu)
- Frame-rate on Raspberry Pi (with egui_glow fallback, if O.12 adds it)
- Latency and inference time per frame

See `plan-opus.md` § O.9 for full context and measurement schedule.

## License

All models in this directory are distributed under the Apache License 2.0, a permissive open-source license that allows commercial use with attribution.

**Attribution**: Models provided by Google MediaPipe team.
