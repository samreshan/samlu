// C-ABI bridge to Bark's Core ML encoder (macOS 15+, for multifunction
// models). Rust prepares the features and decodes the output; this file only
// compiles, loads and runs `BarkEncoder.mlpackage`. Rust calls in from
// blocking worker threads and serializes calls on one handle.

import Accelerate
import CoreML
import Foundation

private enum BarkStatus {
    static let ok: Int32 = 0
    static let failed: Int32 = 1
}

private let features = 128

private func write(_ message: String, to out: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>) {
    out.pointee = strdup(message)
}

/// One loaded encoder, one Core ML function per window length in seconds.
private final class BarkEncoder {
    let windows: [Int32: MLModel]

    init(windows: [Int32: MLModel]) {
        self.windows = windows
    }
}

/// Compiles the package next to itself once, so later loads skip it. A
/// read-only folder still works; it just compiles again next time.
@available(macOS 15.0, *)
private func compiledModel(in dir: URL) throws -> URL {
    let compiled = dir.appendingPathComponent("BarkEncoder.mlmodelc")
    if FileManager.default.fileExists(atPath: compiled.appendingPathComponent("coremldata.bin").path) {
        return compiled
    }
    let fresh = try MLModel.compileModel(at: dir.appendingPathComponent("BarkEncoder.mlpackage"))
    do {
        try? FileManager.default.removeItem(at: compiled)
        try FileManager.default.moveItem(at: fresh, to: compiled)
        return compiled
    } catch {
        return fresh
    }
}

@available(macOS 15.0, *)
private func computeUnits(_ code: Int32) -> MLComputeUnits {
    switch code {
    case 1: return .cpuAndGPU
    case 2: return .cpuOnly
    default: return .cpuAndNeuralEngine
    }
}

@_cdecl("samlu_bark_available")
public func samluBarkAvailable() -> Bool {
    guard #available(macOS 15.0, *) else { return false }
    return true
}

/// Returns a retained handle, or nil with a message in `out`.
@_cdecl("samlu_bark_load")
public func samluBarkLoad(
    _ dir: UnsafePointer<CChar>,
    _ units: Int32,
    _ windowList: UnsafePointer<Int32>,
    _ windowCount: Int32,
    _ out: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
) -> UnsafeMutableRawPointer? {
    guard #available(macOS 15.0, *) else {
        write("Underdog Bark needs macOS 15 or later.", to: out)
        return nil
    }
    let folder = URL(fileURLWithPath: String(cString: dir), isDirectory: true)
    do {
        let compiled = try compiledModel(in: folder)
        var windows: [Int32: MLModel] = [:]
        for seconds in UnsafeBufferPointer(start: windowList, count: Int(windowCount)) {
            let configuration = MLModelConfiguration()
            configuration.computeUnits = computeUnits(units)
            configuration.functionName = "w\(seconds)"
            windows[seconds] = try MLModel(contentsOf: compiled, configuration: configuration)
        }
        return Unmanaged.passRetained(BarkEncoder(windows: windows)).toOpaque()
    } catch {
        write("Could not load Underdog Bark: \(error.localizedDescription)", to: out)
        return nil
    }
}

@_cdecl("samlu_bark_release")
public func samluBarkRelease(_ handle: UnsafeMutableRawPointer?) {
    guard let handle else { return }
    Unmanaged<BarkEncoder>.fromOpaque(handle).release()
}

private func convertToHalf(_ source: UnsafePointer<Float>, _ target: UnsafeMutableRawPointer, _ count: Int) {
    var input = vImage_Buffer(
        data: UnsafeMutableRawPointer(mutating: source), height: 1, width: vImagePixelCount(count),
        rowBytes: count * 4)
    var output = vImage_Buffer(data: target, height: 1, width: vImagePixelCount(count), rowBytes: count * 2)
    vImageConvert_PlanarFtoPlanar16F(&input, &output, 0)
}

private func convertFromHalf(_ source: UnsafeRawPointer, _ target: UnsafeMutablePointer<Float>, _ count: Int) {
    var input = vImage_Buffer(
        data: UnsafeMutableRawPointer(mutating: source), height: 1, width: vImagePixelCount(count),
        rowBytes: count * 2)
    var output = vImage_Buffer(data: target, height: 1, width: vImagePixelCount(count), rowBytes: count * 4)
    vImageConvert_Planar16FtoPlanarF(&input, &output, 0)
}

/// Runs the `window`-second function on `frames × 128` features with the
/// first `valid` frames unmasked, writing up to `capacity` floats of
/// `encoded` (`outFrames × outWidth`) into `output`.
@_cdecl("samlu_bark_encode")
public func samluBarkEncode(
    _ handle: UnsafeMutableRawPointer,
    _ window: Int32,
    _ input: UnsafePointer<Float>,
    _ frames: Int32,
    _ valid: Int32,
    _ output: UnsafeMutablePointer<Float>,
    _ capacity: Int,
    _ outFrames: UnsafeMutablePointer<Int32>,
    _ outWidth: UnsafeMutablePointer<Int32>,
    _ out: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
) -> Int32 {
    guard #available(macOS 15.0, *) else {
        write("Underdog Bark needs macOS 15 or later.", to: out)
        return BarkStatus.failed
    }
    return encode(handle, window, input, frames, valid, output, capacity, outFrames, outWidth, out)
}

@available(macOS 15.0, *)
private func encode(
    _ handle: UnsafeMutableRawPointer,
    _ window: Int32,
    _ input: UnsafePointer<Float>,
    _ frames: Int32,
    _ valid: Int32,
    _ output: UnsafeMutablePointer<Float>,
    _ capacity: Int,
    _ outFrames: UnsafeMutablePointer<Int32>,
    _ outWidth: UnsafeMutablePointer<Int32>,
    _ out: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
) -> Int32 {
    let encoder = Unmanaged<BarkEncoder>.fromOpaque(handle).takeUnretainedValue()
    guard let model = encoder.windows[window] else {
        write("Underdog Bark has no \(window)-second window.", to: out)
        return BarkStatus.failed
    }
    do {
        let count = Int(frames)
        let featureArray = try MLMultiArray(
            shape: [1, NSNumber(value: count), NSNumber(value: features)], dataType: .float16)
        featureArray.withUnsafeMutableBytes { buffer, _ in
            convertToHalf(input, buffer.baseAddress!, count * features)
        }
        let mask = try MLMultiArray(shape: [1, NSNumber(value: count)], dataType: .int32)
        mask.withUnsafeMutableBufferPointer(ofType: Int32.self) { buffer, _ in
            for index in 0..<count {
                buffer[index] = index < Int(valid) ? 1 : 0
            }
        }
        let inputs = try MLDictionaryFeatureProvider(dictionary: ["features": featureArray, "mask": mask])
        let result = try model.prediction(from: inputs)
        guard let encoded = result.featureValue(for: "encoded")?.multiArrayValue,
            encoded.shape.count == 3
        else {
            write("Underdog Bark returned no encoder output.", to: out)
            return BarkStatus.failed
        }
        let rows = encoded.shape[1].intValue
        let width = encoded.shape[2].intValue
        guard rows * width <= capacity else {
            write("Underdog Bark returned more output than expected.", to: out)
            return BarkStatus.failed
        }
        let strides = encoded.strides.map(\.intValue)
        if encoded.dataType == .float16 && strides[2] == 1 && strides[1] == width {
            encoded.withUnsafeBytes { buffer in
                convertFromHalf(buffer.baseAddress!, output, rows * width)
            }
        } else {
            for row in 0..<rows {
                for column in 0..<width {
                    output[row * width + column] = encoded[[0, row, column] as [NSNumber]].floatValue
                }
            }
        }
        outFrames.pointee = Int32(rows)
        outWidth.pointee = Int32(width)
        return BarkStatus.ok
    } catch {
        write("Underdog Bark failed: \(error.localizedDescription)", to: out)
        return BarkStatus.failed
    }
}

@_cdecl("samlu_bark_free")
public func samluBarkFree(_ pointer: UnsafeMutablePointer<CChar>?) {
    free(pointer)
}
