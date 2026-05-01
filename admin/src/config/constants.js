module.exports = {
  TABLE_NAME: {
    YAD_USER: "_user",
    YAD_USER_DETAILS: "_user_details",
    YAD_ADMIN_USER: '_admin_user'
  },

  PHONE_NUMBER: {
    APPROVED: "approved",
  },

  RESPONSE_STATUS: {
    CONTINUE: {
      code: 100,
      value: "Continue",
    },
    SWITCHING_PROTOCOLS: {
      code: 101,
      value: "Switching Protocols",
    },
    PROCESSING: {
      code: 102,
      value: "Processing",
    },
    OK: {
      code: 200,
      value: "OK",
    },
    CREATED: {
      code: 201,
      value: "Created",
    },
    ACCEPTED: {
      code: 202,
      value: "Accepted",
    },
    NON_AUTHORITATIVE_INFORMATION: {
      code: 203,
      value: "Non Authoritative Information",
    },
    NO_CONTENT: {
      code: 204,
      value: "No Content",
    },
    RESET_CONTENT: {
      code: 205,
      value: "Reset Content",
    },
    PARTIAL_CONTENT: {
      code: 206,
      value: "Partial Content",
    },
    MULTI_STATUS: {
      code: 207,
      value: "Multi-Status",
    },
    MULTIPLE_CHOICES: {
      code: 300,
      value: "Multiple Choices",
    },
    MOVED_PERMANENTLY: {
      code: 301,
      value: "Moved Permanently",
    },
    MOVED_TEMPORARILY: {
      code: 302,
      value: "Moved Temporarily",
    },
    SEE_OTHER: {
      code: 303,
      value: "See Other",
    },
    NOT_MODIFIED: {
      code: 304,
      value: "Not Modified",
    },
    USE_PROXY: {
      code: 305,
      value: "Use Proxy",
    },
    TEMPORARY_REDIRECT: {
      code: 307,
      value: "Temporary Redirect",
    },
    PERMANENT_REDIRECT: {
      code: 308,
      value: "Bad Request",
    },
    BAD_REQUEST: {
      code: 400,
      value: "Accepted",
    },
    UNAUTHORIZED: {
      code: 401,
      value: "Unauthorized",
    },
    PAYMENT_REQUIRED: {
      code: 402,
      value: "Payment Required",
    },
    FORBIDDEN: {
      code: 403,
      value: "Forbidden",
    },
    NOT_FOUND: {
      code: 404,
      value: "Not Found",
    },
    METHOD_NOT_ALLOWED: {
      code: 405,
      value: "Method Not Allowed",
    },
    NOT_ACCEPTABLE: {
      code: 406,
      value: "Not Acceptable",
    },
    PROXY_AUTHENTICATION_REQUIRED: {
      code: 407,
      value: "Proxy Authentication Required",
    },
    REQUEST_TIMEOUT: {
      code: 408,
      value: "Timeout",
    },
    CONFLICT: {
      code: 409,
      value: "Conflict",
    },
    GONE: {
      code: 410,
      value: "Gone",
    },
    LENGTH_REQUIRED: {
      code: 411,
      value: "Length Required",
    },
    PRECONDITION_FAILED: {
      code: 412,
      value: "Precondition Failed",
    },
    REQUEST_TOO_LONG: {
      code: 413,
      value: "Request Entity Too Large",
    },
    REQUEST_URI_TOO_LONG: {
      code: 414,
      value: "Request-URI Too Long",
    },
    UNSUPPORTED_MEDIA_TYPE: {
      code: 415,
      value: "Unsupported Media Type",
    },
    REQUESTED_RANGE_NOT_SATISFIABLE: {
      code: 416,
      value: "Requested Range Not Satisfiable",
    },
    EXPECTATION_FAILED: {
      code: 417,
      value: "Expectation Failed",
    },
    IM_A_TEAPOT: {
      code: 418,
      value: `I'm a teapot`,
    },
    INSUFFICIENT_SPACE_ON_RESOURCE: {
      code: 419,
      value: "Insufficient Space on Resource",
    },
    METHOD_FAILURE: {
      code: 420,
      value: "Method Failure",
    },
    MISDIRECTED_REQUEST: {
      code: 421,
      value: "Misdirected Request",
    },
    UNPROCESSABLE_ENTITY: {
      code: 422,
      value: "Unprocessable Entity",
    },
    LOCKED: {
      code: 423,
      value: "Locked",
    },
    FAILED_DEPENDENCY: {
      code: 424,
      value: "Failed Dependency",
    },
    PRECONDITION_REQUIRED: {
      code: 428,
      value: "Precondition Required",
    },
    TOO_MANY_REQUESTS: {
      code: 429,
      value: "Too Many Requests",
    },
    REQUEST_HEADER_FIELDS_TOO_LARGE: {
      code: 431,
      value: "Request Header Fields Too Large",
    },
    UNAVAILABLE_FOR_LEGAL_REASONS: {
      code: 451,
      value: "Unavailable For Legal Reasons",
    },
    INTERNAL_SERVER_ERROR: {
      code: 500,
      value: "Internal Server Error",
    },
    NOT_IMPLEMENTED: {
      code: 501,
      value: "Not Implemented",
    },
    BAD_GATEWAY: {
      code: 502,
      value: "Bad Gateway",
    },
    SERVICE_UNAVAILABLE: {
      code: 503,
      value: "Service Unavailable",
    },
    GATEWAY_TIMEOUT: {
      code: 504,
      value: "Gateway Timeout",
    },
    HTTP_VERSION_NOT_SUPPORTED: {
      code: 505,
      value: "HTTP Version Not Supported",
    },
    INSUFFICIENT_STORAGE: {
      code: 507,
      value: "Insufficient Storage",
    },
    NETWORK_AUTHENTICATION_REQUIRED: {
      code: 511,
      value: "Network Authentication Required",
    },
  },
  STATUS_CODE: {
    202: "Accepted",
    502: "Bad Gateway",
    400: "Bad Request",
    409: "Conflict",
    100: "Continue",
    201: "Created",
    417: "Expectation Failed",
    424: "Failed Dependency",
    403: "Forbidden",
    504: "Gateway Timeout",
    410: "Gone",
    505: "HTTP Version Not Supported",
    418: "I'm a teapot",
    419: "Insufficient Space on Resource",
    507: "Insufficient Storage",
    500: "Internal Server Error",
    411: "Length Required",
    423: "Locked",
    420: "Method Failure",
    405: "Method Not Allowed",
    301: "Moved Permanently",
    302: "Moved Temporarily",
    207: "Multi-Status",
    300: "Multiple Choices",
    511: "Network Authentication Required",
    204: "No Content",
    203: "Non Authoritative Information",
    406: "Not Acceptable",
    404: "Not Found",
    501: "Not Implemented",
    304: "Not Modified",
    200: "OK",
    206: "Partial Content",
    402: "Payment Required",
    308: "Permanent Redirect",
    412: "Precondition Failed",
    428: "Precondition Required",
    102: "Processing",
    407: "Proxy Authentication Required",
    431: "Request Header Fields Too Large",
    408: "Request Timeout",
    413: "Request Entity Too Large",
    414: "Request-URI Too Long",
    416: "Requested Range Not Satisfiable",
    205: "Reset Content",
    303: "See Other",
    503: "Service Unavailable",
    101: "Switching Protocols",
    307: "Temporary Redirect",
    429: "Too Many Requests",
    401: "Unauthorized",
    451: "Unavailable For Legal Reasons",
    422: "Unprocessable Entity",
    415: "Unsupported Media Type",
    305: "Use Proxy",
    421: "Misdirected Request",
  },

  REDIS: {
    YAD_EMAIL_VERIFICATION: "YAD_EMAIL_VERIFICATION",
    YAD_ACTIVE_LOGINS: "YAD_ACTIVE_LOGINS",
  },

  ERROR_MESSAGE: {
    JWT_EXPIRED:"jwt expired"
  },

  USER_DETAILS_MODEL_DEFAULT:{
    appLanguage: "ENGLISH",
    contentTranslation: "en",
    verified:false,
    isPrivate: false
  },

  HEALTH_CHECK_TIMEOUT: 5000
};
