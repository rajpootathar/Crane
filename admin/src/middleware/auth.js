const Authentication = require("../utils/authentication");
const { response } = require("../utils/response-handler");

const { RESPONSE_STATUS, ERROR_MESSAGE } = require("../config/constants");

module.exports = async (req, res, next) => {
  try {
    const isAuthorized = await Authentication.ValidateSignature(req);
    if (isAuthorized) {
      return next();
    }
    return response(res, RESPONSE_STATUS.UNAUTHORIZED.code, false, null, null);
  } catch (error) {
    if (error.message === ERROR_MESSAGE.JWT_EXPIRED) {
      error.message = "Session expired. Please log in again";
      error.status = RESPONSE_STATUS.UNAUTHORIZED.code;
    }
    next(error);
  }
};
