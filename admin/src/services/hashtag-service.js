const Axios = require("axios");
const { AUTH_PROXY } = require("../config");
class HashtagService {
  constructor() {}
  async getAllHashtags(page, pageSize) {
    try {
      const headers = {
        isAdmin: true,
      };
      const result = await Axios.get(
        `${AUTH_PROXY}/get-all-hashtags-with-count?page=${page}&pageSize=${pageSize}`,
        { headers }
      );
      const { data } = result.data;
      return data;
    } catch (error) {
      throw new Error(error);
    }
  }
  async deleteHashtag(id) {
    try {
      const result = await Axios.delete(`${AUTH_PROXY}/hashtag/${id}`);
      const { data } = result.data;
      return data;
    } catch (error) {
      throw new Error(error);
    }
  }
  async getHashtagDetails(id) {
    try {
      const headers = {
        isAdmin: true,
      };
      const result = await Axios.get(`${AUTH_PROXY}/hashtag/${id}/details`, {
        headers,
      });
      const { data } = result.data;
      return data;
    } catch (error) {
      throw new Error(error);
    }
  }
  async getUserHashtagDetails(id) {
    try {
      const result = await Axios.get(`${AUTH_PROXY}/hashtag/${id}`);
      const { data } = result.data;
      return data;
    } catch (error) {
      throw new Error(error);
    }
  }
}
module.exports = HashtagService;
