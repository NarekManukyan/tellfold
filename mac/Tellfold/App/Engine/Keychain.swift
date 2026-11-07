import Foundation
import Security

/// API keys live in the login Keychain as generic passwords: service `dev.tellfold.mac.api-keys`, account = the
/// environment variable name the engine reads (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `TELLFOLD_KEY_<PROVIDER>`).
/// They are passed to the engine process as environment variables only; never written to disk or logs.
struct Keychain: Sendable {
    let service: String

    static let apiKeys = Keychain(service: "dev.tellfold.mac.api-keys")

    static let anthropicAccount = "ANTHROPIC_API_KEY"
    static let openAIAccount = "OPENAI_API_KEY"

    /// `TELLFOLD_KEY_<PROVIDER_ID>`, the same rule as `tellfold.settings.key_env_for`: upper-cased, each run of
    /// characters other than A-Z / 0-9 replaced by one `_`, leading and trailing `_` removed.
    static func envName(forProvider id: String) -> String {
        var out = ""
        var pendingUnderscore = false
        for scalar in id.uppercased().unicodeScalars {
            let isWord = scalar.isASCII && (("A"..."Z").contains(scalar) || ("0"..."9").contains(scalar))
            if isWord {
                if pendingUnderscore, !out.isEmpty { out.append("_") }
                pendingUnderscore = false
                out.unicodeScalars.append(scalar)
            } else {
                pendingUnderscore = true
            }
        }
        return "TELLFOLD_KEY_" + out
    }

    /// A valid POSIX environment variable name.
    static func isValidEnvName(_ name: String) -> Bool {
        guard let first = name.unicodeScalars.first, !name.isEmpty else { return false }
        let head = CharacterSet.letters.union(CharacterSet(charactersIn: "_"))
        let rest = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "_"))
        return first.isASCII && head.contains(first)
            && name.unicodeScalars.allSatisfy { $0.isASCII && rest.contains($0) }
    }

    enum KeychainError: Error, LocalizedError {
        case status(OSStatus)

        var errorDescription: String? {
            switch self {
            case .status(let s):
                let message = SecCopyErrorMessageString(s, nil) as String? ?? "error \(s)"
                return "Keychain: \(message)"
            }
        }
    }

    private func baseQuery(_ account: String? = nil) -> [String: Any] {
        var q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
        ]
        if let account { q[kSecAttrAccount as String] = account }
        return q
    }

    func string(for account: String) -> String? {
        var q = baseQuery(account)
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var item: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &item) == errSecSuccess, let data = item as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    func contains(_ account: String) -> Bool {
        var q = baseQuery(account)
        q[kSecReturnAttributes as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var item: CFTypeRef?
        return SecItemCopyMatching(q as CFDictionary, &item) == errSecSuccess
    }

    /// Stores `value` for `account`; an empty or nil value deletes it.
    func set(_ value: String?, for account: String) throws {
        let trimmed = value?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !trimmed.isEmpty else {
            try delete(account)
            return
        }
        let data = Data(trimmed.utf8)
        let update: [String: Any] = [kSecValueData as String: data]
        var status = SecItemUpdate(baseQuery(account) as CFDictionary, update as CFDictionary)
        if status == errSecItemNotFound {
            var add = baseQuery(account)
            add[kSecValueData as String] = data
            add[kSecAttrLabel as String] = "Tellfold: \(account)"
            add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
            status = SecItemAdd(add as CFDictionary, nil)
        }
        guard status == errSecSuccess else { throw KeychainError.status(status) }
    }

    func delete(_ account: String) throws {
        let status = SecItemDelete(baseQuery(account) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw KeychainError.status(status) }
    }

    /// Every account stored under this service.
    func accounts() -> [String] {
        var q = baseQuery()
        q[kSecReturnAttributes as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitAll
        var items: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &items) == errSecSuccess,
              let list = items as? [[String: Any]] else { return [] }
        return list.compactMap { $0[kSecAttrAccount as String] as? String }.sorted()
    }

    /// Account → secret for every stored key whose account is a valid environment variable name.
    func environment() -> [String: String] {
        var env: [String: String] = [:]
        for account in accounts() where Self.isValidEnvName(account) {
            if let value = string(for: account) { env[account] = value }
        }
        return env
    }
}
