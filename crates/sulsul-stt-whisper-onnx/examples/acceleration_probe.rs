//! 모델이나 사용자 설정을 읽지 않고 DirectML 장치와 엔진의 지원 범위를 확인합니다.

fn main() {
    let devices = sulsul_stt_sensevoice::detected_devices();
    println!(
        "DirectML 포함 여부: {}",
        sulsul_stt_sensevoice::accel::DIRECTML_BUILT
    );
    println!("DirectML GPU 목록: {:?}", devices.gpus);
    println!("DirectML NPU 목록: {:?}", devices.npus);
    println!("Whisper ONNX 지원 장치: GPU(인코더만), CPU");
    println!("Whisper ONNX NPU: 미지원 (실제 NPU 추론 경로 미검증)");
    println!("Windows 장치 관리자에 있는 NPU가 DirectML 목록에는 없을 수 있습니다.");
    println!("이 결과는 장치 감지 결과이며 실제 모델 추론 성공을 뜻하지 않습니다.");
}
