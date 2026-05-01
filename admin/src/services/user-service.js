const Axios = require("axios");

const { AUTH_PROXY, FEED_PROXY } = require("../config");
class UserService {
  constructor() {}

  async getUserList(page, pageSize, deletedAt) {
    try {
      const result = await Axios.get(
        `${AUTH_PROXY}/admin-users?page=${page}&pageSize=${pageSize}&deletedAt=${deletedAt}`
      );
      const { data } = result.data;
      return data;
    } catch (error) {
      throw new Error(error);
    }
  }
  async getUserProfile(userId) {
    try {
      const result = await Axios.get(`${AUTH_PROXY}/user-profile/${userId}`);
      const { data } = result.data;
      return data;
    } catch (error) {
      console.log(error);
      throw new Error(error);
    }
  }
  async getUserPosts(userId) {
    try {
      const result = await Axios.get(`${FEED_PROXY}/user-posts/${userId}`);
      const { data } = result.data;
      return data;
    } catch (error) {
      throw new Error(error);
    }
  }
  async getUserComments(userId) {
    try {
      const result = await Axios.get(`${FEED_PROXY}/comments/user/${userId}`);
      const { data } = result.data;
      return data;
    } catch (error) {
      throw new Error(error);
    }
  }
  async deleteUserAccount(userId) {
    try {
      const result = await Axios.delete(`${AUTH_PROXY}/account/${userId}`);
      const { data } = result.data;
      return data;
    } catch (error) {
      throw new Error(error);
    }
  }
  async ActivateDisableUserAccount(userId) {
    try {
      const result = await Axios.put(`${AUTH_PROXY}/status/${userId}`);
      const { data } = result.data;
      return data;
    } catch (error) {
      throw new Error(error);
    }
  }
}

module.exports = UserService;
