// C-ABI bridge to Apple's SpeechAnalyzer (macOS 26+). Rust calls these from
// blocking worker threads. Every entry point checks availability first, so
// the weakly linked Speech and Swift concurrency symbols are never touched on
// older systems.

import AVFoundation
import Foundation
import Speech

private enum BridgeStatus {
    static let ok: Int32 = 0
    static let failed: Int32 = 1
    static let notInstalled: Int32 = 2
    static let unsupportedLocale: Int32 = 3
    static let unavailable: Int32 = 4
}

private final class ResultBox: @unchecked Sendable {
    var status = BridgeStatus.failed
    var message = ""
}

/// Rust calls in from a blocking worker thread, never the main thread, so
/// waiting on a semaphore here cannot deadlock the UI.
@available(macOS 26.0, *)
private func blockOn(_ body: @escaping @Sendable () async -> (Int32, String)) -> (Int32, String) {
    let box = ResultBox()
    let semaphore = DispatchSemaphore(value: 0)
    Task.detached {
        let (status, message) = await body()
        box.status = status
        box.message = message
        semaphore.signal()
    }
    semaphore.wait()
    return (box.status, box.message)
}

private func write(_ message: String, to out: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>) {
    out.pointee = strdup(message)
}

@available(macOS 26.0, *)
private func resolveLocale(_ identifier: String) async -> Locale? {
    let requested = identifier.isEmpty ? Locale.current : Locale(identifier: identifier)
    return await SpeechTranscriber.supportedLocale(equivalentTo: requested)
}

@available(macOS 26.0, *)
private func isInstalled(_ locale: Locale) async -> Bool {
    let wanted = locale.identifier(.bcp47)
    return await SpeechTranscriber.installedLocales.contains { $0.identifier(.bcp47) == wanted }
}

@_cdecl("samlu_apple_speech_available")
public func samluAppleSpeechAvailable() -> Bool {
    guard #available(macOS 26.0, *) else { return false }
    return SpeechTranscriber.isAvailable
}

@_cdecl("samlu_apple_install")
public func samluAppleInstall(
    _ localeId: UnsafePointer<CChar>,
    _ out: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
) -> Int32 {
    guard #available(macOS 26.0, *) else {
        write("Apple on-device speech requires macOS 26 or later.", to: out)
        return BridgeStatus.unavailable
    }
    let identifier = String(cString: localeId)
    let (status, message) = blockOn {
        guard let locale = await resolveLocale(identifier) else {
            return (BridgeStatus.unsupportedLocale, identifier)
        }
        let transcriber = SpeechTranscriber(locale: locale, preset: .transcription)
        do {
            if let request = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) {
                try await request.downloadAndInstall()
            }
            return (BridgeStatus.ok, locale.identifier(.bcp47))
        } catch {
            return (BridgeStatus.failed, error.localizedDescription)
        }
    }
    write(message, to: out)
    return status
}

@_cdecl("samlu_apple_transcribe")
public func samluAppleTranscribe(
    _ path: UnsafePointer<CChar>,
    _ localeId: UnsafePointer<CChar>,
    _ terms: UnsafePointer<CChar>,
    _ out: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
) -> Int32 {
    guard #available(macOS 26.0, *) else {
        write("Apple on-device speech requires macOS 26 or later.", to: out)
        return BridgeStatus.unavailable
    }
    let url = URL(fileURLWithPath: String(cString: path))
    let identifier = String(cString: localeId)
    let vocabulary = String(cString: terms).split(separator: "\n").map(String.init)
    let (status, message) = blockOn {
        guard let locale = await resolveLocale(identifier) else {
            return (BridgeStatus.unsupportedLocale, identifier)
        }
        guard await isInstalled(locale) else {
            return (BridgeStatus.notInstalled, locale.identifier(.bcp47))
        }
        do {
            let transcriber = SpeechTranscriber(locale: locale, preset: .transcription)
            let analyzer = SpeechAnalyzer(modules: [transcriber])
            if !vocabulary.isEmpty {
                let context = AnalysisContext()
                context.contextualStrings[.general] = vocabulary
                try await analyzer.setContext(context)
            }
            let collector = Task { () async throws -> String in
                var text = ""
                for try await result in transcriber.results {
                    let phrase = String(result.text.characters)
                        .trimmingCharacters(in: .whitespacesAndNewlines)
                    if phrase.isEmpty { continue }
                    text += text.isEmpty ? phrase : " " + phrase
                }
                return text
            }
            let file = try AVAudioFile(forReading: url)
            if let last = try await analyzer.analyzeSequence(from: file) {
                try await analyzer.finalizeAndFinish(through: last)
            } else {
                await analyzer.cancelAndFinishNow()
            }
            return (BridgeStatus.ok, try await collector.value)
        } catch {
            return (BridgeStatus.failed, error.localizedDescription)
        }
    }
    write(message, to: out)
    return status
}

@_cdecl("samlu_apple_free")
public func samluAppleFree(_ pointer: UnsafeMutablePointer<CChar>?) {
    free(pointer)
}
