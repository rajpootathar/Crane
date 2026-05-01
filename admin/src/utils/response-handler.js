const {STATUS_CODE} = require('../config/constants.js')

const response = async (res, code, success, data, message) => {
  if (!message && code) {
    message = STATUS_CODE[code] || '';
  }
  return res.status(code).json({
    success: success,
    data: data,
    message: message,
  });
};

module.exports = { response };
