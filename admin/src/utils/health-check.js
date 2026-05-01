const axios = require("axios");
const {
  AUTH_PROXY,
  USER_PROXY,
  NOTIFICATION_PROXY,
  FOLLOW_PROXY,
} = require("../config");
const { HEALTH_CHECK_TIMEOUT } = require("../config/constants");

const checkServerHealth = async (url, timeout) => {
  try {
    const response = await axios.get(url, { timeout });
    return response.status === 200;
  } catch (error) {
    return false;
  }
};

exports.checkAuthServerHealth = () => {
  const SERVER_URL = `${AUTH_PROXY}/health-check`;
  checkServerHealth(SERVER_URL, HEALTH_CHECK_TIMEOUT)
    .then((result) => {
      return result;
    })
    .catch((error) => {
      return error;
    });
};
exports.checkUserServerHealth = () => {
  const SERVER_URL = `${USER_PROXY}/health-check`;
  checkServerHealth(SERVER_URL, HEALTH_CHECK_TIMEOUT)
    .then((result) => {
      return result;
    })
    .catch((error) => {
      return error;
    });
};
exports.checkNotificationServerHealth = () => {
  const SERVER_URL = `${NOTIFICATION_PROXY}/health-check`;
  checkServerHealth(SERVER_URL, HEALTH_CHECK_TIMEOUT)
    .then((result) => {
      return result;
    })
    .catch((error) => {
      return error;
    });
};
exports.checkFollowServerHealth = () => {
  const SERVER_URL = `${FOLLOW_PROXY}/health-check`;
  checkServerHealth(SERVER_URL, HEALTH_CHECK_TIMEOUT)
    .then((result) => {
      return result;
    })
    .catch((error) => {
      return error;
    });
};
