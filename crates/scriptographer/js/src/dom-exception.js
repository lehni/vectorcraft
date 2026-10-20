// `DOMException` for scripts outside a browser: an `Error` with a `name`, and
// the legacy `code` of the names that had one, as `INDEX_SIZE_ERR` and the
// other constants have them.

// The names in the order of their codes, from 1, and their constants:
const constants = [
  ['IndexSizeError', 'INDEX_SIZE_ERR'],
  ['DOMStringSizeError', 'DOMSTRING_SIZE_ERR'],
  ['HierarchyRequestError', 'HIERARCHY_REQUEST_ERR'],
  ['WrongDocumentError', 'WRONG_DOCUMENT_ERR'],
  ['InvalidCharacterError', 'INVALID_CHARACTER_ERR'],
  ['NoDataAllowedError', 'NO_DATA_ALLOWED_ERR'],
  ['NoModificationAllowedError', 'NO_MODIFICATION_ALLOWED_ERR'],
  ['NotFoundError', 'NOT_FOUND_ERR'],
  ['NotSupportedError', 'NOT_SUPPORTED_ERR'],
  ['InUseAttributeError', 'INUSE_ATTRIBUTE_ERR'],
  ['InvalidStateError', 'INVALID_STATE_ERR'],
  ['SyntaxError', 'SYNTAX_ERR'],
  ['InvalidModificationError', 'INVALID_MODIFICATION_ERR'],
  ['NamespaceError', 'NAMESPACE_ERR'],
  ['InvalidAccessError', 'INVALID_ACCESS_ERR'],
  ['ValidationError', 'VALIDATION_ERR'],
  ['TypeMismatchError', 'TYPE_MISMATCH_ERR'],
  ['SecurityError', 'SECURITY_ERR'],
  ['NetworkError', 'NETWORK_ERR'],
  ['AbortError', 'ABORT_ERR'],
  ['URLMismatchError', 'URL_MISMATCH_ERR'],
  ['QuotaExceededError', 'QUOTA_EXCEEDED_ERR'],
  ['TimeoutError', 'TIMEOUT_ERR'],
  ['InvalidNodeTypeError', 'INVALID_NODE_TYPE_ERR'],
  ['DataCloneError', 'DATA_CLONE_ERR']
]
const codes = new Map(constants.map(([name], index) => [name, index + 1]))

export class DOMException extends Error {
  #name

  constructor(message = '', name = 'Error') {
    super(message)
    this.#name = String(name)
  }

  get name() {
    return this.#name
  }

  get code() {
    return codes.get(this.#name) ?? 0
  }
}

for (const [index, [, constant]] of constants.entries()) {
  for (const target of [DOMException, DOMException.prototype]) {
    Object.defineProperty(target, constant, {
      value: index + 1,
      enumerable: true
    })
  }
}

Object.defineProperty(DOMException.prototype, Symbol.toStringTag, {
  value: 'DOMException',
  configurable: true
})
