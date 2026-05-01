const { response } = require("../utils/response-handler");
const { RESPONSE_STATUS } = require("../config/constants");
const PostService = require("../services/post-service");


const PostsController = () => {};
const service = new PostService();
PostsController.fetchAllPosts = async (req, res,next) => {
    const {page=1,pageSize=10}=req.query;
    try {
        const data = await service.getAllPosts(page,pageSize);
        return response(res, RESPONSE_STATUS.OK.code, true, data, null);
    }
    catch (error) {
        next(error);
    }
}

PostsController.deletePost = async (req, res,next) => {
    const {id}= req.params;
    try {
        const data = await service.deletePost(id);
        return response(res, RESPONSE_STATUS.OK.code, true, data, null);
    }
    catch (error) {
        console.log(error);
        next(error);
    }
}
PostsController.getPostDetails = async (req, res,next) => {
    const {id}= req.params;
    try {
        const data = await service.getPostDetails(id);
        return response(res, RESPONSE_STATUS.OK.code, true, data, null);
    }
    catch (error) {
        console.log(error);
        next(error);
    }
}

module.exports = PostsController;
