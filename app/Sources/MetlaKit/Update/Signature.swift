import Foundation
import Security

/// Code signatures, read with Security.framework. An update's one trust
/// check: the new bundle must satisfy the running app's own designated
/// requirement, so only a build signed by the same certificate passes;
/// it is the rule macOS ties the Full Disk Access grant to.
public enum Signature {
    /// What a signature says of its code: its designated requirement, in
    /// the code signing requirement language, and whether it is ad-hoc (or
    /// none at all). An ad-hoc signature's requirement is its own hash:
    /// no other build can satisfy it.
    public struct Own: Equatable, Sendable {
        public let requirement: String?
        public let adHoc: Bool
    }

    /// Strict validation, every architecture, nested code, no symbolic
    /// link that leads out of the bundle. No revocation check: it would
    /// fail an install on a flaky network, and the requirement does not
    /// depend on it.
    static let flags = SecCSFlags(
        rawValue: kSecCSStrictValidate | kSecCSCheckAllArchitectures | kSecCSCheckNestedCode | kSecCSRestrictSymlinks)

    /// The running app's own signature.
    public static func running() -> Own {
        var code: SecCode?
        var file: SecStaticCode?
        guard SecCodeCopySelf([], &code) == errSecSuccess, let code,
            SecCodeCopyStaticCode(code, [], &file) == errSecSuccess, let file
        else {
            return Own(requirement: nil, adHoc: true)
        }
        return own(file)
    }

    /// The signature of the code at `url`: a bundle or an executable.
    public static func of(_ url: URL) -> Own {
        guard let file = staticCode(url) else {
            return Own(requirement: nil, adHoc: true)
        }
        return own(file)
    }

    /// Whether the code at `url` is validly signed and satisfies
    /// `requirement`; false for a requirement that does not compile.
    public static func satisfies(_ url: URL, _ requirement: String) -> Bool {
        var compiled: SecRequirement?
        guard SecRequirementCreateWithString(requirement as CFString, [], &compiled) == errSecSuccess, let compiled,
            let file = staticCode(url)
        else {
            return false
        }
        return SecStaticCodeCheckValidityWithErrors(file, flags, compiled, nil) == errSecSuccess
    }

    private static func staticCode(_ url: URL) -> SecStaticCode? {
        var file: SecStaticCode?
        guard SecStaticCodeCreateWithPath(url as CFURL, [], &file) == errSecSuccess else {
            return nil
        }
        return file
    }

    private static func own(_ file: SecStaticCode) -> Own {
        var info: CFDictionary?
        let read = SecCodeCopySigningInformation(file, SecCSFlags(rawValue: kSecCSSigningInformation), &info)
        let flags = ((info as? [String: Any])?[kSecCodeInfoFlags as String] as? NSNumber)?.uint32Value
        let adHocOrLinker = SecCodeSignatureFlags.adhoc.rawValue | 0x20000  // kSecCodeSignatureLinkerSigned
        let adHoc = read != errSecSuccess || flags.map { $0 & adHocOrLinker != 0 } ?? true
        var requirement: SecRequirement?
        var text: CFString?
        guard SecCodeCopyDesignatedRequirement(file, [], &requirement) == errSecSuccess, let requirement,
            SecRequirementCopyString(requirement, [], &text) == errSecSuccess
        else {
            return Own(requirement: nil, adHoc: adHoc)
        }
        return Own(requirement: text as String?, adHoc: adHoc)
    }
}
