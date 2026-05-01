const DashBoardService = require("../services/dashboard-service");
const { RESPONSE_STATUS } = require("../config/constants");
const { response } = require("../utils/response-handler");



const DashboardController = () => {};
const _service = new DashBoardService();

DashboardController.getUserMatrix = async (req, res, next) => {
    try {
      const data = await _service.getUserMetrics(req.body);
      return response(res, RESPONSE_STATUS.OK.code, true, data, null);
    } catch (error) {
      console.log(error)
      next(error);
    }
};

DashboardController.getContentMetrics = async (req, res, next) => {
  try {
    const data = await _service.getContentMetrics(req.body);
    return response(res, RESPONSE_STATUS.OK.code, true, data, null);
  } catch (error) {
    next(error);
  }
};

DashboardController.recentUserList = async (req, res, next) => {
  try {
    const data = await _service.recentUserList(req.body);
    return response(res, RESPONSE_STATUS.OK.code, true, data, null);
  } catch (error) {
    next(error);
  }
};

DashboardController.getUploadedPostMetrix = async (req, res, next) => {
  try {
    const data = await _service.getUploadedPost(req.body);
    return response(res, RESPONSE_STATUS.OK.code, true, data, null);
  } catch (error) {
    next(error);
  }
};


DashboardController.getUserRegistrationMetrix = async (req, res, next) => {
  try {
    const data = await _service.getUserRegistration(req.body);
    return response(res, RESPONSE_STATUS.OK.code, true, data, null);
  } catch (error) {
    next(error);
  }
};


module.exports = DashboardController;