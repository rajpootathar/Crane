const UserService = require("../services/user-service");
const { response } = require("../utils/response-handler");
const { RESPONSE_STATUS } = require("../config/constants");

const UsersController = () => {};
const service = new UserService();

// Fetch All Users
UsersController.fetchUsers = async (req, res, next) => {
  const { page = 1, pageSize = 10, deletedAt = false } = req.query;

  try {
    const data = await service.getUserList(
      page,
      pageSize,
      !(deletedAt.toString() === "false")
    );
    return response(res, RESPONSE_STATUS.OK.code, true, data, null);
  } catch (error) {
    next(error);
  }
};

// Get User Profile
UsersController.getUserProfile = async (req, res, next) => {
  const { id } = req.params;
  if (!id) {
    return response(
      res,
      RESPONSE_STATUS.BAD_REQUEST.code,
      false,
      null,
      "user Id is required"
    );
  }
  try {
    const data = await service.getUserProfile(id);
    return response(res, RESPONSE_STATUS.OK.code, true, data, null);
  } catch (error) {
    console.log(error);
    next(error);
  }
};
UsersController.deleteUserAccount = async (req, res, next) => {
  const { userId } = req.params;
  if (!userId) {
    return response(
      res,
      RESPONSE_STATUS.BAD_REQUEST.code,
      false,
      null,
      "user Id is required"
    );
  }
  try {
    const data = await service.deleteUserAccount(userId);
    return response(res, RESPONSE_STATUS.OK.code, true, data, null);
  } catch (error) {
    next(error);
  }
};
UsersController.ActivateDisableUserAccount = async (req, res, next) => {
  const { userId } = req.params;
  if (!userId) {
    return response(
      res,
      RESPONSE_STATUS.BAD_REQUEST.code,
      false,
      null,
      "user Id is required"
    );
  }
  try {
    const data = await service.ActivateDisableUserAccount(userId);
    return response(res, RESPONSE_STATUS.OK.code, true, data, null);
  } catch (error) {
    next(error);
  }
};
module.exports = UsersController;
