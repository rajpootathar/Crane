const { response } = require("../utils/response-handler");
const { RESPONSE_STATUS } = require("../config/constants");
const ReelService = require("../services/reel-service");

const ReelsController = () => {};
const service = new ReelService();

ReelsController.fetchAllReels = async (req, res, next) => {
  const { page = 1, pageSize = 10 } = req.query;
  try {
    const data = await service.getAllReels(page, pageSize);
    return response(res, RESPONSE_STATUS.OK.code, true, data, null);
  } catch (error) {
    next(error);
  }
};

ReelsController.deleteReel = async (req, res, next) => {
  const { id } = req.params;
  try {
    const data = await service.deleteReel(id);
    return response(res, RESPONSE_STATUS.OK.code, true, data, null);
  } catch (error) {
    console.log(error);
    next(error);
  }
};
ReelsController.getReelDetails = async (req, res, next) => {
  const { id } = req.params;
  try {
    const data = await service.getReelDetails(id);
    return response(res, RESPONSE_STATUS.OK.code, true, data, null);
  } catch (error) {
    console.log(error);
    next(error);
  }
};

module.exports = ReelsController;
