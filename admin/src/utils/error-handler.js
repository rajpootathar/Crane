const logger = require("./logger");
const { RESPONSE_STATUS } = require("../config/constants");
const { response } = require("../utils/response-handler");

const ErrorHandler = async (error, req, res, next) => {
  logger
    .apiLoggerHandler(
      error.module ? error.module : "AuthService",
      error.type ? error.type : "error"
    )
    [error.type ? error.type : "error"]({
      Method: req.method ? req.method : "",
      URL: req.url ? req.url : "",
      StatusCode:
        error && error.status
          ? error.status
          : RESPONSE_STATUS.INTERNAL_SERVER_ERROR.code,
      Message: error.message,
    });

  return response(
    res,
    error && error.status
      ? error.status
      : RESPONSE_STATUS.INTERNAL_SERVER_ERROR.code,
    false,
    null,
    error && error.message
  );
};

module.exports = ErrorHandler;
