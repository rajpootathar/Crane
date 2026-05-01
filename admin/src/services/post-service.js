const Axios = require("axios");
const { FEED_PROXY } = require("../config");
class PostService {
  constructor() {
    //this.repository = new UserRepository();
  }
  async getAllPosts(page, pageSize) {
    try {
      const result = await Axios.get(
        `${FEED_PROXY}/posts?page=${page}&pageSize=${pageSize}`
      );
      const { data } = result.data;
      return data;
    } catch (error) {
      throw new Error(error);
    }
  }
  async deletePost(id) {
    try {
      const result = await Axios.delete(`${FEED_PROXY}/posts/${id}`);
      const { data } = result.data;
      return data;
    } catch (error) {
      throw new Error(error);
    }
  }
  async getPostDetails(id) {
    try {
      const result = await Axios.get(`${FEED_PROXY}/posts/${id}`);
      const { data } = result.data;
      return data;
    } catch (error) {
      throw new Error(error);
    }
  }
  async getUserPostsDetails(id) {
    try {
      const result = await Axios.get(`${FEED_PROXY}/posts/${id}`);
      const { data } = result.data;
      return data;
    } catch (error) {
      throw new Error(error);
    }
  }
}
module.exports = PostService;
